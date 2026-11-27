// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! One page about one strategy, from what the record already holds. Computes nothing itself.
//! Usage: bijective-report <store> <strategy-program> <confidence-in-the-fixed-scale>

use bijective_common::clock::as_utc;
use bijective_evaluation::evidence::preregistration::Horizon;
use bijective_evaluation::measure::{Basis, measure};
use bijective_evaluation::promotion::path::{every_strategy, gather, landed, runs_of};
use bijective_evaluation::promotion::steps::{Known, resolve, walk};

fn list(root: &std::path::Path) -> std::process::ExitCode {
    let recorded = match every_strategy(root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("bijective-report: the record could not be read: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    println!("# Strategies in `{}`", root.display());
    println!();
    if recorded.is_empty() {
        println!("None. Nothing has run against this store.");
        return std::process::ExitCode::SUCCESS;
    }

    println!("| last run | strategy | runs | run as | name |");
    println!("|---|---|---|---|---|");
    for one in &recorded {
        println!(
            "| {} | `{}` | {} | {} | {} |",
            as_utc(one.last_touched_ns),
            &one.strategy.to_hex()[..12],
            one.runs,
            one.classes.join(", "),
            // A digest cannot be turned back into a name. Say so rather than show a blank.
            one.program.as_deref().unwrap_or(" - not recorded")
        );
    }
    println!();
    println!("Most recent first. For any of them: `bijective-report <store> <name> <confidence>`.");
    std::process::ExitCode::SUCCESS
}

/// The stream separates a paper session from a live one. The class alone does not, so a paper
/// figure cannot be read as live. A run that admitted nothing says so.
fn where_it_landed(
    root: &std::path::Path,
    run: bijective_event_store::digest::Digest,
) -> (String, String) {
    match landed(root, run) {
        Ok(admissions) if admissions.is_empty() => {
            ("nothing admitted".to_string(), " - ".to_string())
        }
        Ok(admissions) => {
            let mut streams: Vec<&str> = admissions.iter().map(|a| a.proposals.as_str()).collect();
            streams.sort_unstable();
            streams.dedup();
            let mut markets: Vec<&str> = admissions.iter().map(|a| a.market.as_str()).collect();
            markets.sort_unstable();
            markets.dedup();
            (format!("`{}`", streams.join("`, `")), format!("`{}`", markets.join("`, `")))
        }
        Err(e) => (format!("unreadable: {e}"), " - ".to_string()),
    }
}

fn main() -> std::process::ExitCode {
    let a: Vec<String> = std::env::args().skip(1).collect();

    if let [root] = a.as_slice() {
        return list(std::path::Path::new(root));
    }

    let [root, program, confidence] = a.as_slice() else {
        eprintln!("usage: bijective-report <store> [<strategy-program> <confidence>]");
        return std::process::ExitCode::from(2);
    };
    let root = std::path::Path::new(root);
    let Ok(confidence) = confidence.parse::<i128>() else {
        eprintln!("bijective-report: the confidence is a critical value in the fixed scale");
        return std::process::ExitCode::from(2);
    };

    let (strategy, known) = resolve(root, program);
    let name = program.rsplit('/').next().unwrap_or(program);
    print_header(name, strategy, root, known);

    let runs = match runs_of(root, strategy) {
        Ok(runs) => runs,
        Err(e) => {
            println!("The record could not be read: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    print_runs(root, name, &runs);
    print_claim(root, &runs);
    print_measurements(root, name, &runs);
    print_standing(root, program, confidence);
    std::process::ExitCode::SUCCESS
}

/// Strategy name, identity and how it was found. A lookup by file name or path is stated, so the
/// page is never about a strategy the operator did not ask for.
fn print_header(
    name: &str,
    strategy: bijective_event_store::digest::Digest,
    root: &std::path::Path,
    known: Known,
) {
    println!("# {name}");
    println!();
    println!("Strategy `{}` - store `{}`", &strategy.to_hex()[..16], root.display());
    // A silent answer about the other reading would be a page about a strategy the operator did not ask for.
    match known {
        Known::Declared => println!(),
        Known::ByFileName => {
            println!();
            println!("Found by the name its file suggests. That is a lookup, not its identity.");
            println!();
        }
        Known::ByPath => {
            println!();
            println!("Found by its path. Nothing has run under that name.");
            println!();
        }
        Known::Neither => {
            println!();
            println!("Nothing has run under this name or this path.");
            println!();
        }
    }
}

/// One row per run, oldest first, and a warning when the program's bytes changed between runs.
fn print_runs(
    root: &std::path::Path,
    name: &str,
    runs: &[(bijective_event_store::digest::Digest, String)],
) {
    println!("## Runs");
    println!();
    let mut bytes_seen: Vec<bijective_event_store::digest::Digest> = Vec::new();
    if runs.is_empty() {
        println!("None. This node has no record of {name} ever running.");
    } else {
        // Oldest first, so a reader sees whether the figures moved.
        println!(
            "| when | run | bytes | class | proposals landed on | market | ended | reproduced |"
        );
        println!("|---|---|---|---|---|---|---|---|");
        for (run, class) in runs {
            let (stream, market) = where_it_landed(root, *run);
            match gather(root, *run) {
                Ok(Some(g)) => {
                    // Every entry, not the newest: a changed file hides between two runs of one specification.
                    bytes_seen.extend(g.entries.iter().filter_map(|e| e.content));
                    println!(
                        "| {} | `{}` | {} | {class} | {stream} | {market} | {:?} in {} ms | {} |",
                        as_utc(g.entry.started_ns),
                        &run.to_hex()[..12],
                        g.entry.content.map_or(" - not recorded".to_string(), |c| format!(
                            "`{}`",
                            &c.to_hex()[..8]
                        )),
                        g.entry.exit,
                        (g.entry.ended_ns - g.entry.started_ns) / 1_000_000,
                        g.reproduction
                    );
                }
                // Said, not skipped: a silent hole reads as a complete page.
                Ok(None) => println!(
                    "| - | `{}` | - | {class} | {stream} | {market} | no entry | - |",
                    &run.to_hex()[..12]
                ),
                Err(e) => println!(
                    "| - | `{}` | - | {class} | {stream} | {market} | unreadable: {e} | - |",
                    &run.to_hex()[..12]
                ),
            }
        }
    }

    println!();
    println!("Oldest first. A figure is worth reading against the one before it, not on its own.");

    // The bytes say which version ran, and a figure from one version is not about another.
    bytes_seen.sort_unstable_by_key(|d| d.to_hex());
    bytes_seen.dedup();
    if bytes_seen.len() > 1 {
        println!();
        println!(
            "**The file changed between these runs: {} different version(s) ran under one name.** \
             A figure measured on one of them is not a figure about another.",
            bytes_seen.len()
        );
    }
    println!();
}

/// The hypothesis and decision rule registered for the newest research run.
fn print_claim(root: &std::path::Path, runs: &[(bijective_event_store::digest::Digest, String)]) {
    if let Some((run, _)) = runs.iter().rev().find(|(_, class)| class == "research")
        && let Ok(Some(g)) = gather(root, *run)
    {
        println!("## The claim");
        println!();
        match &g.registration {
            Some(registered) => {
                let rule = registered.rule();
                println!("> {}", registered.hypothesis());
                println!();
                println!("Measured as `{}`, threshold {}.", rule.measure, rule.threshold);
            }
            None => println!("None declared. A run with no declared claim has no decision rule."),
        }
        println!();
    }
}

/// The figures `measure` recorded for each run. Read back, never recomputed.
fn print_measurements(
    root: &std::path::Path,
    name: &str,
    runs: &[(bijective_event_store::digest::Digest, String)],
) {
    println!("## What was measured");
    println!();
    let mut any = false;
    for (run, class) in runs {
        let Ok(m) = measure(root, *run) else { continue };
        any = true;
        println!("### `{}` - {class}", &run.to_hex()[..12]);
        println!();
        println!(
            "{} proposal(s) priced, {} unpriceable - {} observation(s) - dispersion {}",
            m.placed.len(),
            m.unplaced.len(),
            m.observations,
            m.dispersion
        );
        println!();
        println!("- floor: {}", m.floor);
        println!("- clearance: {}", m.clearance);
        // Spelled out: no horizon declared and no holding needed read differently to an operator.
        println!(
            "- horizon: {}",
            match m.horizon {
                Horizon::After { positions } => {
                    format!("marked {positions} market position(s) after the decision")
                }
                Horizon::NotHeld => "not held - the claim is about a cost or a spread".to_string(),
                Horizon::NotDeclared => "not declared".to_string(),
            }
        );
        // Said: a figure measured where it was fitted is not the same figure, and the promotion stages refuses on it.
        println!(
            "- basis: {}",
            match m.basis {
                Basis::HeldOut => "held out, on the folds the claim declared",
                Basis::Everything => "everything, because the claim declared no folds",
            }
        );
        if m.measured.is_empty() {
            println!("- figures: none - nothing it proposed crossed the touch");
        } else {
            println!();
            println!("| measure | value |");
            println!("|---|---|");
            for figure in &m.measured {
                println!("| {} | {} |", figure.measure, figure.value);
            }
        }
        println!();
    }
    if !any {
        println!("Nothing. No run of {name} has figures on this node.");
        println!();
    }
}

/// Each promotion stage, what stopped it and the next step.
fn print_standing(root: &std::path::Path, program: &str, confidence: i128) {
    println!("## Where it stands");
    println!();
    for step in walk(root, program, confidence) {
        println!("- **{}** - {}", step.stage, step.said);
        if let Some(next) = step.next {
            println!("  - next: {next}");
        }
    }
    println!();
    println!(
        "Confidence {confidence} in the fixed scale, as declared on the command that made this."
    );
}
