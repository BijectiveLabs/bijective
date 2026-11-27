// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! `bijective-journal`: journal retention for an operator.
//!
//! Usage:
//!   bijective-journal prune --store <dir> --venue <name> --archive <dir>
//!                           --price-scale <n> --quantity-scale <n>
//!
//! Removes journal segments a verified archive holds. Safe while the data engine runs.
//! Exit codes: 0 pruned (possibly nothing), 1 refused, 2 the invocation itself is wrong.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use bijective_archive::ArchiveConfig;
use bijective_data_engine::retention::prune_journal;

const PROGRAM: &str = "bijective-journal";
const FLAGS: [&str; 5] = ["--store", "--venue", "--archive", "--price-scale", "--quantity-scale"];

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("prune") => {}
        Some(other) => return usage(&format!("{other} is not a command: prune")),
        None => return usage("a command is required: prune"),
    }
    let flags = match flags(args) {
        Ok(f) => f,
        Err(e) => return usage(&e),
    };
    let (store, venue, archive) = match inputs(&flags) {
        Ok(i) => i,
        Err(e) => return usage(&e),
    };
    match prune_journal(&store, &venue, &archive) {
        Ok(done) => {
            let begins = done.pruned.horizon.map_or("0".into(), |h| h.to_string());
            println!(
                "pruned {} segment(s), {} entr(ies), {} byte(s); archived through entry {}; the journal begins at entry {begins}",
                done.pruned.segments,
                done.pruned.entries,
                done.pruned.bytes,
                done.archived_through.journal_seq,
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{PROGRAM}: nothing removed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn flags(mut args: impl Iterator<Item = String>) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    while let Some(flag) = args.next() {
        if !FLAGS.contains(&flag.as_str()) {
            return Err(format!("{flag} is not a flag; flags are {}", FLAGS.join(", ")));
        }
        let value = args.next().ok_or_else(|| format!("{flag} has no value"))?;
        out.insert(flag, value);
    }
    Ok(out)
}

fn inputs(flags: &BTreeMap<String, String>) -> Result<(PathBuf, String, ArchiveConfig), String> {
    let get = |name: &str| flags.get(name).cloned().ok_or_else(|| format!("{name} is required"));
    let scale = |name: &str| -> Result<u8, String> {
        get(name)?.parse().map_err(|_| format!("{name} is not a whole number of decimal places"))
    };
    let venue = get("--venue")?;
    let archive = ArchiveConfig {
        root: PathBuf::from(get("--archive")?),
        venue: venue.clone(),
        price_scale: scale("--price-scale")?,
        quantity_scale: scale("--quantity-scale")?,
    };
    Ok((PathBuf::from(get("--store")?), venue, archive))
}

fn usage(why: &str) -> ExitCode {
    eprintln!("{PROGRAM}: {why}");
    ExitCode::from(2)
}
