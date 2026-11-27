// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! `bijective-archive`: verify an archive against its manifest, or compact one partition.
//!
//! Usage:
//!   bijective-archive verify  --root <dir> --venue <name> --price-scale <n> --quantity-scale <n>
//!   bijective-archive compact --root <dir> --venue <name> --price-scale <n> --quantity-scale <n>
//!                             --table <book|facts|gaps> --date <YYYY-MM-DD>
//!
//! Exit codes: 0 sound or compacted, 1 a finding or refusal, 2 the invocation itself is wrong.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use bijective_archive::{ArchiveConfig, compact, verify};

const PROGRAM: &str = "bijective-archive";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return usage("a command is required: verify or compact");
    };
    let flags = match flags(args) {
        Ok(f) => f,
        Err(e) => return usage(&e),
    };
    let config = match config_of(&flags) {
        Ok(c) => c,
        Err(e) => return usage(&e),
    };
    match command.as_str() {
        "verify" => run_verify(&config),
        "compact" => run_compact(&config, &flags),
        other => usage(&format!("{other} is not a command: verify or compact")),
    }
}

fn flags(mut args: impl Iterator<Item = String>) -> Result<BTreeMap<String, String>, String> {
    let known = ["--root", "--venue", "--price-scale", "--quantity-scale", "--table", "--date"];
    let mut out = BTreeMap::new();
    while let Some(flag) = args.next() {
        if !known.contains(&flag.as_str()) {
            return Err(format!("{flag} is not a flag; flags are {}", known.join(", ")));
        }
        let value = args.next().ok_or_else(|| format!("{flag} has no value"))?;
        out.insert(flag, value);
    }
    Ok(out)
}

fn config_of(flags: &BTreeMap<String, String>) -> Result<ArchiveConfig, String> {
    let get = |name: &str| flags.get(name).cloned().ok_or_else(|| format!("{name} is required"));
    let scale = |name: &str| -> Result<u8, String> {
        get(name)?.parse().map_err(|_| format!("{name} is not a whole number of decimal places"))
    };
    Ok(ArchiveConfig {
        root: PathBuf::from(get("--root")?),
        venue: get("--venue")?,
        price_scale: scale("--price-scale")?,
        quantity_scale: scale("--quantity-scale")?,
    })
}

fn run_verify(config: &ArchiveConfig) -> ExitCode {
    match verify(config) {
        Ok(report) => {
            println!(
                "{} file(s), {} book row(s), {} fact row(s), {} gap row(s), {} finding(s)",
                report.files,
                report.book_rows,
                report.fact_rows,
                report.gap_rows,
                report.findings.len()
            );
            for finding in &report.findings {
                println!("FINDING  {finding}");
            }
            if report.is_sound() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        Err(e) => refuse(&e.to_string()),
    }
}

fn run_compact(config: &ArchiveConfig, flags: &BTreeMap<String, String>) -> ExitCode {
    let table: &'static str = match flags.get("--table").map(String::as_str) {
        Some("book") => "book",
        Some("gaps") => "gaps",
        Some("facts") => "facts",
        _ => return usage("--table is required: book, facts or gaps"),
    };
    let Some(date) = flags.get("--date") else {
        return usage("--date is required: YYYY-MM-DD");
    };
    match compact(config, table, date) {
        Ok(done) => {
            println!("{} file(s) merged into {} ({} row(s))", done.inputs, done.path, done.rows);
            ExitCode::SUCCESS
        }
        Err(e) => refuse(&e.to_string()),
    }
}

fn refuse(why: &str) -> ExitCode {
    eprintln!("{PROGRAM}: {why}");
    ExitCode::FAILURE
}

fn usage(why: &str) -> ExitCode {
    eprintln!("{PROGRAM}: {why}");
    ExitCode::from(2)
}
