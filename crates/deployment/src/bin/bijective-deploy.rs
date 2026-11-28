// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The deployment kit's operator commands.
//! `verify-units` regenerates and diffs, so a hand-edited unit is detectable.

use std::path::{Path, PathBuf};

use bijective_deployment::{
    Checksums, Deployment, NodeState, artefact, assess_node, compose, generate,
};
use bijective_event_store::format::{
    SEGMENT_HEADER_LEN, Verdict, entry_header_len, parse_segment_header,
};
use bijective_event_store::store::verify_segment;

fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!(
            "usage: bijective-deploy <status|generate|compose|notices|record|verify-units|verify-artefact|restore-drill> ..."
        );
        return std::process::ExitCode::from(2);
    };
    let rest: Vec<String> = args.collect();

    match command.as_str() {
        "generate" => run_generate(&rest),
        "compose" => run_compose(&rest),
        "verify-units" => run_verify_units(&rest),
        "record" => run_record(&rest),
        "verify-artefact" => run_verify_artefact(&rest),
        "restore-drill" => run_restore_drill(&rest),
        "status" => run_status(&rest),
        "notices" => run_notices(&rest),
        other => {
            eprintln!("bijective-deploy: unknown command {other}");
            std::process::ExitCode::from(2)
        }
    }
}

fn load(path: &Path) -> Result<Deployment, String> {
    let raw = std::fs::read(path).map_err(|e| format!("manifest unreadable: {e}"))?;
    Deployment::parse(&raw).map_err(|e| e.to_string())
}

fn run_generate(args: &[String]) -> std::process::ExitCode {
    let [manifest, out_dir] = args else {
        eprintln!("usage: bijective-deploy generate <manifest.json> <out-dir>");
        return std::process::ExitCode::from(2);
    };

    let deployment = match load(Path::new(manifest)) {
        Ok(d) => d,
        Err(e) => return fail(&e),
    };
    let generated = generate(&deployment);

    let out = PathBuf::from(out_dir);
    if let Err(e) = std::fs::create_dir_all(&out) {
        return fail(&format!("cannot write to {out_dir}: {e}"));
    }
    for (name, body) in &generated.units {
        if let Err(e) = std::fs::write(out.join(name), body) {
            return fail(&format!("cannot write {name}: {e}"));
        }
    }

    // Printed, not applied: enabling a unit changes an operator's host and the output must stay inspectable.
    for unit in &generated.enable {
        println!("enable {unit}");
    }
    for name in generated.units.keys() {
        if !generated.enable.contains(name) {
            println!("install-stopped {name}");
        }
    }
    std::process::ExitCode::SUCCESS
}

/// Prints the composition. Writing it is the operator's redirect, so the command changes nothing.
fn run_compose(args: &[String]) -> std::process::ExitCode {
    let [manifest] = args else {
        eprintln!("usage: bijective-deploy compose <manifest.json>");
        return std::process::ExitCode::from(2);
    };
    match load(Path::new(manifest)) {
        Ok(deployment) => {
            print!("{}", compose(&deployment));
            std::process::ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

fn run_verify_units(args: &[String]) -> std::process::ExitCode {
    let [manifest, unit_dir] = args else {
        eprintln!("usage: bijective-deploy verify-units <manifest.json> <unit-dir>");
        return std::process::ExitCode::from(2);
    };

    let deployment = match load(Path::new(manifest)) {
        Ok(d) => d,
        Err(e) => return fail(&e),
    };
    let generated = generate(&deployment);
    let dir = PathBuf::from(unit_dir);
    let mut drifted = 0;

    for (name, expected) in &generated.units {
        match std::fs::read_to_string(dir.join(name)) {
            Ok(found) if &found == expected => {}
            Ok(_) => {
                println!("DRIFT   {name} differs from what the manifest generates");
                drifted += 1;
            }
            Err(_) => {
                println!("MISSING {name}");
                drifted += 1;
            }
        }
    }

    if drifted == 0 {
        println!("PASS  {} unit(s) match the manifest", generated.units.len());
        std::process::ExitCode::SUCCESS
    } else {
        println!();
        println!("A unit that differs from the manifest is running something the release did not");
        println!("test, and the difference is invisible until a restart.");
        std::process::ExitCode::FAILURE
    }
}

/// Record a release's checksums with the same traversal `verify_artefact` uses.
fn run_record(args: &[String]) -> std::process::ExitCode {
    let [root, platform_version] = args else {
        eprintln!("usage: bijective-deploy record <artefact-dir> <platform-version>");
        return std::process::ExitCode::from(2);
    };

    let root = PathBuf::from(root);
    let checksums = match artefact::checksums_for(&root, platform_version) {
        Ok(c) => c,
        Err(e) => return fail(&format!("the artefact could not be read: {e}")),
    };
    if checksums.files.is_empty() {
        return fail("the artefact holds no files to record");
    }

    let rendered = match serde_json::to_vec_pretty(&checksums) {
        Ok(r) => r,
        Err(e) => return fail(&format!("the manifest could not be rendered: {e}")),
    };
    if let Err(e) = std::fs::write(root.join(artefact::CHECKSUMS_FILE), rendered) {
        return fail(&format!("the manifest could not be written: {e}"));
    }

    println!("recorded {} file(s) for {platform_version}", checksums.files.len());
    std::process::ExitCode::SUCCESS
}

fn run_verify_artefact(args: &[String]) -> std::process::ExitCode {
    let [root] = args else {
        eprintln!("usage: bijective-deploy verify-artefact <artefact-dir>");
        return std::process::ExitCode::from(2);
    };

    let root = PathBuf::from(root);
    let raw = match std::fs::read(root.join(artefact::CHECKSUMS_FILE)) {
        Ok(r) => r,
        Err(e) => return fail(&format!("checksum manifest unreadable: {e}")),
    };
    let checksums: Checksums = match serde_json::from_slice(&raw) {
        Ok(c) => c,
        Err(_) => return fail("the checksum manifest could not be read"),
    };

    match artefact::verify_artefact(&root, &checksums) {
        Ok(n) => {
            println!("PASS  {n} file(s) verified before anything was unpacked");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("artefact refused: {e}")),
    }
}

/// Write the third-party notices file from the attestation.
fn run_notices(args: &[String]) -> std::process::ExitCode {
    let [attestation, registry, out] = args else {
        eprintln!(
            "usage: bijective-deploy notices <attestation.tsv> <cargo-registry-src> <out.md>"
        );
        return std::process::ExitCode::from(2);
    };

    let raw = match std::fs::read_to_string(attestation) {
        Ok(r) => r,
        Err(e) => return fail(&format!("attestation unreadable: {e}")),
    };
    let packages = match bijective_deployment::parse_attestation(&raw) {
        Ok(p) => p,
        Err(e) => return fail(&e.to_string()),
    };

    let texts = bijective_deployment::collect_texts(
        &packages,
        Path::new(registry),
        Path::new(attestation).parent().unwrap_or(Path::new(".")),
    );

    match bijective_deployment::render(&packages, &texts) {
        Ok(body) => match std::fs::write(out, &body) {
            Ok(()) => {
                println!(
                    "wrote {} package(s) and {} licence text(s) to {out}",
                    packages.len(),
                    bijective_deployment::distinct_licences(&packages).len()
                );
                std::process::ExitCode::SUCCESS
            }
            Err(e) => fail(&format!("could not write {out}: {e}")),
        },
        Err(e) => fail(&e.to_string()),
    }
}

/// Node state as one answer. Exit 0 only when ready. A degraded operational program does not change it.
fn run_status(args: &[String]) -> std::process::ExitCode {
    let [manifest, run_dir] = args else {
        eprintln!("usage: bijective-deploy status <manifest.json> <run-dir>");
        return std::process::ExitCode::from(2);
    };

    let deployment = match load(Path::new(manifest)) {
        Ok(d) => d,
        Err(e) => return fail(&e),
    };

    // Judged against the same clock interface the node stamps with.
    let now = bijective_common::clock::Clock::now_ns(&bijective_common::clock::WallClock::new());
    let node = assess_node(&deployment, Path::new(run_dir), now);

    println!("{:?}  {}", node.state, node.because);
    for p in &node.programs {
        let health = match (p.health, p.lapsed) {
            (Some(h), _) => format!("{h:?}"),
            (None, true) => "Lapsed".to_string(),
            (None, false) => "Silent".to_string(),
        };
        println!("  {:<12} {:<10} {}", format!("{:?}", p.class), health, p.name);
        println!("  {:<23} {}", "", p.detail);
    }

    for degraded in node.degradations() {
        println!();
        println!("DEGRADED  {} is not working and the core does not need it", degraded.name);
        println!("          {}", degraded.detail);
    }

    match node.state {
        NodeState::Ready => std::process::ExitCode::SUCCESS,
        _ => std::process::ExitCode::FAILURE,
    }
}

/// The deployment kit: restore into a fresh directory without a venue, then require a clean verifier report.
fn run_restore_drill(args: &[String]) -> std::process::ExitCode {
    let [backup, into] = args else {
        eprintln!("usage: bijective-deploy restore-drill <backup-dir> <restore-into-dir>");
        return std::process::ExitCode::from(2);
    };

    let backup = PathBuf::from(backup);
    let into = PathBuf::from(into);
    if !target_is_usable(&into) {
        return fail("the restore target is not empty; a drill never writes over existing state");
    }

    let copied = match copy_tree(&backup, &into) {
        Ok(n) => n,
        Err(e) => return fail(&format!("restore failed: {e}")),
    };

    let found = bijective_deployment::host::sweep(&into);
    if found.total() == 0 {
        return fail("the restored store holds no segments, so nothing was verified");
    }
    if !found.is_healthy() {
        return fail(&format!(
            "{} of {} restored segment(s) did not verify; nothing may resume onto this",
            found.damaged,
            found.total()
        ));
    }

    // A torn tail is stated, not swallowed: normal on a live store, but on a restored copy it means a mid-write backup.
    if !found.is_complete() {
        println!(
            "NOTE  {} restored segment(s) end mid-entry, {} byte(s) in total",
            found.with_torn_tail, found.torn_tail_bytes
        );
        println!("      Clean, and not a complete copy. Verify when the backup was taken.");
    }

    // Recovery: a drill that cannot fail is a demonstration, so the refusals run every time.
    match prove_refusals(&into) {
        Ok(proven) => {
            for line in proven {
                println!("PROVEN {line}");
            }
        }
        Err(why) => return fail(&format!("the drill could not be shown to refuse: {why}")),
    }

    println!(
        "PASS  {copied} file(s) restored, {} segment(s) verified clean, no venue involved",
        found.clean
    );
    std::process::ExitCode::SUCCESS
}

/// Build the failures the drill must catch, from the restored copy rather than a fixture.
fn prove_refusals(restored: &Path) -> Result<Vec<String>, String> {
    let mut proven = Vec::new();

    let scratch = restored.with_file_name(".drill-proof");
    let _ = std::fs::remove_dir_all(&scratch);
    copy_tree(restored, &scratch).map_err(|e| format!("could not build a proof copy: {e}"))?;

    let segment = find_segment(&scratch)
        .ok_or("the restored copy holds no segment with an entry, so nothing can be corrupted")?;
    let pristine = std::fs::read(&segment).map_err(|e| format!("proof copy unreadable: {e}"))?;

    // Fixed offsets in the first entry, so every run proves the same two refusals.
    for (damage, offset) in [
        ("a damaged payload byte", first_payload_at(&pristine)?),
        ("a length damaged to run past the end", ENTRY_LENGTH_AT),
    ] {
        let mut bytes = pristine.clone();
        if bytes.len() <= offset {
            let _ = std::fs::remove_dir_all(&scratch);
            return Err("the restored segment is too small to corrupt meaningfully".into());
        }
        bytes[offset] ^= 0xFF;
        std::fs::write(&segment, &bytes).map_err(|e| format!("could not write proof copy: {e}"))?;
        if bijective_deployment::host::sweep(&scratch).is_healthy() {
            let _ = std::fs::remove_dir_all(&scratch);
            return Err(format!("a segment with {damage} verified clean"));
        }
        proven.push(format!("a copy with {damage} is refused"));
    }

    // Exercised against the copy that is still on disk, before it is removed.
    if target_is_usable(&scratch) {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err("a non-empty directory was accepted as a restore target".into());
    }
    proven.push("a non-empty restore target is refused".to_string());

    let _ = std::fs::remove_dir_all(&scratch);
    Ok(proven)
}

/// A restore target must be absent or empty. Separate so the drill can exercise the refusal.
fn target_is_usable(into: &Path) -> bool {
    !into.exists() || std::fs::read_dir(into).map(|mut d| d.next().is_none()).unwrap_or(false)
}

/// The first byte of the first entry's payload, after a header whose length the format fixes.
fn first_payload_at(segment: &[u8]) -> Result<usize, String> {
    let head = parse_segment_header(segment)
        .map_err(|reason| format!("the restored segment has no readable header: {reason:?}"))?;
    Ok(SEGMENT_HEADER_LEN + entry_header_len(head.format))
}
/// The second byte of the first entry's length: flipped, it claims more than the segment holds
/// while staying under the payload bound, which is the damage that resembles a torn tail.
const ENTRY_LENGTH_AT: usize = SEGMENT_HEADER_LEN + 3;

/// The first segment, in path order, that holds at least one whole entry with a payload.
/// Path order, not directory order, so the drill corrupts the same file on every host.
fn find_segment(root: &Path) -> Option<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "seg") {
                found.push(path);
            }
        }
    }
    found.sort();
    found.into_iter().find(|path| {
        let first_payload_len = std::fs::read(path).ok().and_then(|b| {
            b.get(SEGMENT_HEADER_LEN + 2..SEGMENT_HEADER_LEN + 6)
                .map(|l| u32::from_be_bytes([l[0], l[1], l[2], l[3]]))
        });
        matches!(verify_segment(path), Verdict::Clean { entries, .. } if entries > 0)
            && first_payload_len.is_some_and(|len| len > 0)
    })
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<usize> {
    let mut copied = 0;
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copied += copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
            copied += 1;
        }
    }
    Ok(copied)
}

fn fail(why: &str) -> std::process::ExitCode {
    eprintln!("bijective-deploy: {why}");
    std::process::ExitCode::FAILURE
}
