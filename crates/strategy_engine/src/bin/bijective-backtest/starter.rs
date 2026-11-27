// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! `--new`: a strategy that already runs, a sample market when none is recorded, and a project
//! file, so the next command needs no arguments.

use std::path::{Path, PathBuf};

use bijective_strategy_engine::launch::library_for_python;

use crate::{PROGRAM, PROJECT_FILE, Stop, found_market};

/// Writes the strategy at `path`, seeds the sample market into `store` when it holds none, and
/// writes the project file.
pub(crate) fn scaffold(path: &Path, store: Option<&Path>) -> Result<std::process::ExitCode, Stop> {
    if let Err(e) = write_starter(path) {
        eprintln!("{PROGRAM}: {e}");
        return Ok(std::process::ExitCode::FAILURE);
    }
    println!("wrote {}", path.display());

    // The market is a sample and is named one: a first verdict must not need a venue credential or be mistaken for a recording.
    let store = store.map_or_else(|| PathBuf::from("store"), Path::to_path_buf);
    match seed_sample(&store) {
        Ok(Some((stream, events))) => {
            println!(
                "seeded {stream} in {} with {events} book update(s) - a sample, not a venue recording",
                store.display()
            );
            // Named for the scaffolded file: asking the operator would break the no-argument first command.
            let called = path
                .file_stem()
                .map_or_else(|| "strategy".to_string(), |s| s.to_string_lossy().to_string());
            if let Err(e) = write_project(&store, &stream, &called) {
                eprintln!("{PROGRAM}: {e}");
                return Ok(std::process::ExitCode::FAILURE);
            }
            println!("wrote {PROJECT_FILE}, so the next command needs no arguments");
        }
        Ok(None) => {
            println!("store: {} already holds a market; nothing was seeded", store.display())
        }
        Err(e) => {
            eprintln!("{PROGRAM}: {e}");
            return Ok(std::process::ExitCode::FAILURE);
        }
    }

    println!("next:  {PROGRAM} {}", path.display());
    Ok(std::process::ExitCode::SUCCESS)
}

/// The interpreter a scaffolded Python strategy names, written absolute because the run has no PATH.
fn interpreter() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("python3"))
        .find(|candidate| candidate.is_file())
}

/// Named a sample on the stream itself, so a verdict says so in its ledger entry. The spread widens because a claim is sized on dispersion.
/// `Ok(None)` where a market is already recorded: an operator's own data is never added to.
fn seed_sample(store: &Path) -> Result<Option<(String, u64)>, String> {
    if found_market(store).is_ok() {
        return Ok(None);
    }
    std::fs::create_dir_all(store)
        .map_err(|e| format!("{} could not be made: {e}", store.display()))?;

    let stream = bijective_serialization::signal::market::stream("sample");
    let mut writer = bijective_event_store::store::StreamWriter::open(store, &stream)
        .map_err(|e| format!("the sample market could not be opened: {e:?}"))?;

    const EVENTS: u64 = 500;
    for i in 0..EVENTS {
        let update = bijective_serialization::signal::market::BookUpdate {
            instrument: bijective_model::InstrumentId::new("btc-perp"),
            seq: bijective_model::Sequence::Numbered(i),
            venue_ts_ms: 1_788_000_000_000 + i,
            received_at_ns: None,
            bid_price: 2_654_375 + i128::from(i),
            bid_quantity: 1_500,
            ask_price: 2_654_500 + i128::from(i) + 20 * i128::from(i % 17),
            ask_quantity: 900,
            bid_depth: 5,
            ask_depth: 5,
        };
        let payload = bijective_serialization::signal::market::encode(&update)
            .map_err(|e| format!("a sample book could not be encoded: {e:?}"))?;
        writer
            .append(&payload)
            .map_err(|e| format!("the sample market could not be written: {e:?}"))?;
    }

    Ok(Some((stream, EVENTS)))
}

fn write_project(store: &Path, market: &str, name: &str) -> Result<(), String> {
    if Path::new(PROJECT_FILE).exists() {
        return Ok(());
    }
    let body = format!(
        "{{\n  \"name\": \"{name}\",\n  \"store\": \"{}\",\n  \"market\": \"{market}\",\n  \"exchange\": \"stepwise\",\n  \"confidence\": 19600\n}}\n",
        store.display()
    );
    std::fs::write(PROJECT_FILE, body)
        .map_err(|e| format!("{PROJECT_FILE} could not be written: {e}"))
}

/// A strategy that runs: shell unless the name asks for Python, because shell needs nothing installed.
fn write_starter(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Err(format!("{} is already there", path.display()));
    }

    let body = if path.extension().is_some_and(|e| e == "py") {
        let Some(python) = interpreter() else {
            return Err("no python3 on PATH, and a run cannot search for one".to_string());
        };
        if library_for_python().is_none() {
            return Err("this build carries no Python library for a strategy to import".to_string());
        }
        PYTHON_STARTER.replace("{interpreter}", &python.display().to_string())
    } else {
        STARTER.to_string()
    };
    std::fs::write(path, body)
        .map_err(|e| format!("{} could not be written: {e}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut mode = std::fs::metadata(path).map_err(|e| format!("{e}"))?.permissions();
        mode.set_mode(0o755);
        std::fs::set_permissions(path, mode).map_err(|e| format!("{e}"))?;
    }
    Ok(())
}

const PYTHON_STARTER: &str = r#"#!{interpreter}
# A strategy is a program. The supervisor sends it one JSON object per line and reads what it
# writes back. `bijective_strategy` is a convenience over that and holds no rules of its own.
#
# The interpreter is named absolutely because a run's environment is constructed from nothing:
# there is no PATH inside the sandbox to search.

from bijective_strategy import Book, EndOfWindow, answered, events, propose

# A round trip, to see the order path work: buy at the ask on the first book, sell at the bid
# HOLD updates later. Book prices are on the venue's tick, so both orders are. Replace the rule
# with yours.
HOLD = 20
bought_at = None

for event in events():
    if isinstance(event, Book):
        # Prices and quantities are scaled integers carried as text. Compare them as integers;
        # turning one into a float is the conversion this platform does not make.
        at = int(event.position)
        if bought_at is None or at == bought_at + HOLD:
            buying = bought_at is None
            propose(
                desk="alpha",
                instrument=event.instrument,
                side="buy" if buying else "sell",
                quantity_scaled="1",
                price_scaled=event.ask_price if buying else event.bid_price,
                client_id=f"{'buy' if buying else 'sell'}-{at}",
                decided_at_ns="1",
                decided_at_position=event.position,
            )
            if buying:
                bought_at = at

        # Its turn is over. Without this the supervisor waits, and the run reaches its wall bound.
        answered()
    elif isinstance(event, EndOfWindow):
        break
"#;

const STARTER: &str = r#"#!/bin/sh
# A strategy is a program. The supervisor sends it one JSON object per line and reads what it
# writes back, one JSON object per line. Nothing else crosses.
#
# Under the stepwise exchange it is sent one event at a time and must answer each one, which is
# what lets the platform state where a decision was made rather than take a claim.

while IFS= read -r event; do
  case "$event" in
    *'"event":"book"'*)
      # Where this book sits in the market's history. Everything the platform knows about when a
      # decision was made comes from answering the event it was made on.
      at=$(printf '%s' "$event" | sed 's/.*"position":"\([0-9]*\)".*/\1/')

      # Say nothing, or propose. This one proposes on every event, which is a placeholder and not
      # a strategy: replace the condition with yours.
      if [ "$at" = "0" ]; then
        printf '{"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"3000000","client_id":"first","decided_at_ns":"1"}\n'
      fi

      # Its turn is over. Without this the supervisor waits, and the run reaches its wall bound.
      printf '{"emit":"answered"}\n'
      ;;
    *end_of_window*) break ;;
  esac
done
"#;
