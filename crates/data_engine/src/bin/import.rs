// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Import market history from a CSV or JSON Lines file into a store, on the market stream a
//! recording of the profile's venue writes (decision 0016), so backtest and paper read it as
//! they read a recording. The whole file is read and checked before anything is written.
//! Usage: bijective-import <file> --profile <profile.json> --store <dir> --time-unit <ns|us|ms|s>
//!        [--instrument <symbol>] [--columns <field>=<name>,...]
//! Exit codes: 0 imported, 1 a row was refused or the store could not be written, 2 usage.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use bijective_data_engine::import::{Format, Reading, TimeUnit, read_books};
use bijective_event_store::store::{StreamWriter, end_of};
use bijective_venue::profile::{VenueProfile, VenueSymbol};

const PROGRAM: &str = "bijective-import";
const USAGE: &str = "usage: bijective-import <file> --profile <profile.json> --store <dir> \
                     --time-unit <ns|us|ms|s> [--instrument <symbol>] [--columns <field>=<name>,...]";

/// What was asked for.
#[derive(Default)]
struct Asked {
    file: PathBuf,
    profile: Option<PathBuf>,
    store: Option<PathBuf>,
    unit: Option<String>,
    instrument: Option<String>,
    columns: Option<String>,
}

fn parse() -> Result<Asked, String> {
    let mut args = std::env::args().skip(1);
    let mut asked = Asked { file: PathBuf::from(args.next().ok_or(USAGE)?), ..Asked::default() };
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(USAGE)?;
        match flag.as_str() {
            "--profile" => asked.profile = Some(PathBuf::from(value)),
            "--store" => asked.store = Some(PathBuf::from(value)),
            "--time-unit" => asked.unit = Some(value),
            "--instrument" => asked.instrument = Some(value),
            "--columns" => asked.columns = Some(value),
            _ => return Err(USAGE.to_string()),
        }
    }
    Ok(asked)
}

fn main() -> ExitCode {
    let asked = match parse() {
        Ok(asked) => asked,
        Err(detail) => return say(&detail, 2),
    };
    match import(&asked) {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err((detail, code)) => say(&detail, code),
    }
}

fn import(asked: &Asked) -> Result<String, (String, u8)> {
    let usage = |detail: &str| (format!("{detail}\n{USAGE}"), 2);
    let (Some(profile_path), Some(store)) = (&asked.profile, &asked.store) else {
        return Err(usage("--profile and --store are required"));
    };
    let unit = asked.unit.as_deref().and_then(TimeUnit::named).ok_or_else(|| {
        usage("--time-unit is ns, us, ms or s: the unit of the file's time column is never guessed")
    })?;
    let format = format_of(&asked.file).map_err(|d| (d, 2))?;
    let profile = read_profile(profile_path).map_err(|d| (d, 2))?;
    let renamed = renamed(asked.columns.as_deref()).map_err(|d| usage(&d))?;
    let text = std::fs::read_to_string(&asked.file)
        .map_err(|e| (format!("{} could not be read: {e}", asked.file.display()), 1))?;
    let reading = Reading {
        format,
        unit,
        catalogue: &profile.instruments,
        renamed,
        instrument: asked.instrument.as_deref().map(VenueSymbol::new),
    };
    let books = read_books(text.lines(), &reading).map_err(|refused| {
        (format!("{}: {refused}; nothing was written", asked.file.display()), 1)
    })?;
    let stream = bijective_serialization::signal::market::stream(&profile.venue);
    write(store, &stream, &books).map_err(|d| (d, 1))?;
    Ok(format!("imported {} book(s) into {} at {}", books.len(), stream, store.display()))
}

fn format_of(file: &std::path::Path) -> Result<Format, String> {
    match file.extension().and_then(|e| e.to_str()) {
        Some("csv") => Ok(Format::Csv),
        Some("jsonl" | "ndjson") => Ok(Format::JsonLines),
        Some("parquet") => {
            Err("Parquet is not read yet; export the file as CSV or JSON Lines".into())
        }
        _ => Err(format!("{} is not .csv, .jsonl or .ndjson", file.display())),
    }
}

fn read_profile(path: &std::path::Path) -> Result<VenueProfile, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("{} could not be read: {e}", path.display()))?;
    VenueProfile::from_json(&text).map_err(|e| format!("{} is not a profile: {e}", path.display()))
}

/// `--columns time=ts,bid_price=bid`: the canonical field, then the file's name for it.
fn renamed(columns: Option<&str>) -> Result<BTreeMap<String, String>, String> {
    let mut renamed = BTreeMap::new();
    for pair in columns.unwrap_or("").split(',').filter(|p| !p.trim().is_empty()) {
        let Some((field, name)) = pair.split_once('=') else {
            return Err(format!("--columns wants <field>=<name>, not {pair:?}"));
        };
        renamed.insert(field.trim().to_string(), name.trim().to_string());
    }
    Ok(renamed)
}

/// Appends every book to `stream` in a store that holds none of it: a store's market is never
/// added to, so a re-import into a fresh store writes the same bytes.
fn write(
    store: &std::path::Path,
    stream: &str,
    books: &[bijective_serialization::signal::market::BookUpdate],
) -> Result<(), String> {
    std::fs::create_dir_all(store)
        .map_err(|e| format!("{} could not be made: {e}", store.display()))?;
    if matches!(end_of(store, stream), Ok(Some(_))) {
        return Err(format!(
            "{} already holds {stream}; it is never added to, so import into a new store",
            store.display()
        ));
    }
    let mut writer = StreamWriter::open(store, stream)
        .map_err(|e| format!("{stream} could not be opened: {e:?}"))?;
    for book in books {
        let payload = bijective_serialization::signal::market::encode(book)
            .map_err(|e| format!("a book could not be encoded: {e:?}"))?;
        writer.append(&payload).map_err(|e| format!("{stream} could not be written: {e:?}"))?;
    }
    Ok(())
}

fn say(detail: &str, code: u8) -> ExitCode {
    eprintln!("{PROGRAM}: {detail}");
    ExitCode::from(code)
}
