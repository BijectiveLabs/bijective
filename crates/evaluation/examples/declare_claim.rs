// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Declare a claim for a run, before the run. Writes one registration to the stream `standing` reads.
//! Exit codes: 0 declared, 1 refused or not recorded, 2 usage.

use std::collections::BTreeMap;
use std::process::ExitCode;

use bijective_evaluation::evidence::registry::{REGISTRATIONS, declare};
use bijective_evaluation::evidence::{
    DecisionRule, Direction, Fold, Horizon, Positions, PreRegistration,
};
use bijective_event_store::store::StreamWriter;
use bijective_strategy_engine::runs::{Exchange, Live, Research, RunSpec};

const USAGE: &str = "usage: declare_claim <store> <program> <to-position|live> <hypothesis> <measure> \
     at-least|at-most <threshold> <positions|not-held> <folds>\n\
     \x20      folds are learn:until:test:until, comma-separated, or none\n\
     \x20      a bounded run is <position>:whole or <position>:stepwise - the exchange is\n\
     \x20      part of its identity - and `live` declares one for a session";

fn main() -> ExitCode {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [root, program, to_position, hypothesis, measure, direction, threshold, horizon, folds] =
        a.as_slice()
    else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let parsed = (|| {
        let rule = DecisionRule {
            measure: measure.clone(),
            direction: direction_of(direction)?,
            threshold: threshold
                .parse()
                .map_err(|_| format!("a threshold in the fixed scale, not {threshold}"))?,
        };
        Ok::<_, String>((rule, horizon_of(horizon)?, folds_of(folds)?))
    })();
    let (rule, horizon, folds) = match parsed {
        Ok(parsed) => parsed,
        Err(detail) => {
            eprintln!("declare_claim: {detail}");
            return ExitCode::from(2);
        }
    };
    let registration = match register(program, to_position, hypothesis, rule, horizon, folds) {
        Ok(registration) => registration,
        Err(detail) => {
            eprintln!("declare_claim: {detail}");
            return ExitCode::FAILURE;
        }
    };
    let recorded = StreamWriter::open(std::path::Path::new(root), REGISTRATIONS)
        .map_err(|e| format!("the registry could not be opened: {e:?}"))
        .and_then(|mut registry| {
            declare(&registration, 0, &mut registry)
                .map_err(|e| format!("the claim was not recorded: {e:?}"))
        });
    match recorded {
        Ok(at) => {
            println!("declared for {} at position {at}", &registration.run().to_hex()[..16]);
            ExitCode::SUCCESS
        }
        Err(detail) => {
            eprintln!("declare_claim: {detail}");
            ExitCode::FAILURE
        }
    }
}

fn direction_of(text: &str) -> Result<Direction, String> {
    match text {
        "at-least" => Ok(Direction::AtLeast),
        "at-most" => Ok(Direction::AtMost),
        other => Err(format!("a direction is at-least or at-most, not {other}")),
    }
}

fn horizon_of(text: &str) -> Result<Horizon, String> {
    match text {
        "not-held" => Ok(Horizon::NotHeld),
        positions => positions
            .parse()
            .map(|positions| Horizon::After { positions })
            .map_err(|_| format!("a horizon is a count of positions or not-held, not {positions}")),
    }
}

/// `none`, or comma-separated `learn:until:test:until` folds of store positions.
fn folds_of(text: &str) -> Result<Vec<Fold>, String> {
    if text == "none" {
        return Ok(Vec::new());
    }
    text.split(',').map(fold_of).collect()
}

fn fold_of(text: &str) -> Result<Fold, String> {
    let edges: Vec<u64> = text
        .split(':')
        .map(|n| n.parse().map_err(|_| format!("a fold edge is a store position, not {n}")))
        .collect::<Result<_, _>>()?;
    let [learn_from, learn_until, test_from, test_until] = edges.as_slice() else {
        return Err(format!("a fold is four positions, not {text}"));
    };
    Ok(Fold {
        learn: Positions::new(*learn_from, *learn_until),
        test: Positions::new(*test_from, *test_until),
    })
}

/// Class, window and exchange are inside a run's identity. Naming the wrong one registers a run
/// that will never answer it.
fn register(
    program: &str,
    to_position: &str,
    hypothesis: &str,
    rule: DecisionRule,
    horizon: Horizon,
    folds: Vec<Fold>,
) -> Result<PreRegistration, String> {
    let refused = |e| format!("the claim was refused: {e:?}");
    if to_position == "live" {
        let spec = RunSpec::<Live>::declare(
            program.to_string(),
            Vec::new(),
            BTreeMap::new(),
            None,
            Exchange::Stepwise,
        )
        .map_err(|e| format!("the session could not be declared: {e:?}"))?;
        return PreRegistration::declare(hypothesis, rule, horizon, folds, &spec).map_err(refused);
    }
    let (bound, exchange) = match to_position.split_once(':') {
        Some((position, exchange)) => (
            position,
            Exchange::parse(exchange)
                .ok_or(format!("an exchange is whole or stepwise, not {exchange}"))?,
        ),
        None => (to_position, Exchange::Stepwise),
    };
    let bound =
        bound.parse().map_err(|_| format!("a window ends at a store position, not {bound}"))?;
    let spec = RunSpec::<Research>::declare(
        program.to_string(),
        Vec::new(),
        BTreeMap::new(),
        Some(bound),
        exchange,
    )
    .map_err(|e| format!("the run could not be declared: {e:?}"))?;
    PreRegistration::declare(hypothesis, rule, horizon, folds, &spec).map_err(refused)
}
