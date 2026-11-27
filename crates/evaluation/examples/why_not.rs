// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Why a run may not be promoted, answered from the platform's own record.
//! Without a declared critical value the question is asked with nothing measured.

use bijective_evaluation::promotion::path::{Measurement, decide, evidence_of, gather, identities};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(prefix)) = (args.next(), args.next()) else {
        eprintln!(
            "usage: why_not <store-root> <run-digest-or-prefix> [confidence-in-the-fixed-scale]"
        );
        std::process::exit(2);
    };
    let root = std::path::Path::new(&root);

    let known = match identities(root) {
        Ok(known) => known,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let matching: Vec<_> = known.iter().filter(|d| d.to_hex().starts_with(&prefix)).collect();
    let run = match matching.as_slice() {
        [one] => **one,
        [] => {
            println!("no run recorded here begins with {prefix}");
            return;
        }
        many => {
            println!("{} runs begin with {prefix}; say more of it", many.len());
            return;
        }
    };

    // Declared, never defaulted: 19 600 is 1.96 in the fixed scale, and the operator's to say.
    let confidence: Option<i128> = args.next().and_then(|c| c.parse().ok());

    match gather(root, run) {
        Ok(None) => println!("{}: no entry, so this node cannot say it ran", &run.to_hex()[..16]),
        Ok(Some(gathered)) => {
            println!(
                "{}: {} entr(ies), {:?}, registration {}",
                &run.to_hex()[..16],
                gathered.entries.len(),
                gathered.reproduction,
                if gathered.registration.is_some() { "on the registry" } else { "absent" }
            );
            let measured = bijective_evaluation::measure::measure(root, run).ok();
            if let Some(m) = &measured {
                print_measurement(m);
            }

            let measurement = match (&measured, confidence) {
                (Some(m), Some(confidence)) if m.observations > 0 => Measurement::Measured {
                    measured: &m.measured,
                    evidence: evidence_of(&gathered, m, confidence),
                },
                _ => Measurement::NotMeasured,
            };

            match decide(&gathered, &measurement) {
                Ok(promoted) => println!("  promoted: {promoted:?}"),
                Err(refusal) => println!("  refused: {refusal}"),
            }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

/// Observations, priced and unpriced proposals, horizon, basis, clearance and per-fold figures.
fn print_measurement(m: &bijective_evaluation::measure::Measurement) {
    println!(
        "  measured: {} observation(s) of {} placed, floor {}",
        m.observations,
        m.placed.len(),
        m.floor
    );
    for placed in m.placed.iter().take(3) {
        println!("    {placed}");
    }
    for unplaced in &m.unplaced {
        println!("    not priced: {unplaced}");
    }
    println!("    horizon {:?}, basis {:?}, {}", m.horizon, m.basis, m.clearance);
    for fold in &m.folds {
        println!(
            "    fold {} tested {}: {} observation(s), spread {}, gross {:?}",
            fold.fold, fold.test, fold.observations, fold.spread, fold.gross
        );
    }
}
