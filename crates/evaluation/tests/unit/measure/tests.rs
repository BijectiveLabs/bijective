// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a measurement must refuse, and the one property the rest rests on.

use super::*;

use crate::evidence::Horizon;

use crate::evidence::registry::{REGISTRATIONS, declare};
use crate::evidence::{DecisionRule, Direction, PreRegistration};
use bijective_event_store::store::StreamWriter;
use bijective_model::InstrumentId;
use bijective_strategy_engine::ledger::{attribute, record};
use bijective_strategy_engine::runs::ledger::{Admitted, ExitReason, RunLedgerEntry};
use bijective_strategy_engine::runs::{Research, RunSpec};

const MARKET: &str = "market-drill";

fn store(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bijective-measure-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn run_of(byte: u8) -> Digest {
    Digest([byte; 32])
}

fn book(instrument: &str, bid: i128, ask: i128) -> BookUpdate {
    BookUpdate {
        instrument: InstrumentId::new(instrument),
        seq: bijective_model::Sequence::Numbered(1),
        venue_ts_ms: 1_788_000_000_000,
        received_at_ns: None,
        bid_price: bid,
        bid_quantity: 1_500,
        ask_price: ask,
        ask_quantity: 900,
        bid_depth: 5,
        ask_depth: 5,
    }
}

fn proposal(instrument: &str, side: Side, price: Option<i128>) -> Proposal {
    Proposal {
        desk: "alpha".into(),
        instrument: InstrumentId::new(instrument),
        side,
        quantity_scaled: 1,
        price_scaled: price,
        cap_scaled: price.is_none().then_some(3_000_000),
        client_id: "c1".into(),
        decided_at_position: None,
        decided_at_ns: 1,
        venue: None,
        strategy: None,
        terms: Default::default(),
        reduce_only: false,
        parent: None,
        oco: None,
    }
}

/// The admission names the last book, as a bounded run's does: it was sent its whole window first.
fn recorded_run(dir: &std::path::Path, run: Digest, books: &[BookUpdate], proposals: &[Proposal]) {
    let last = books.len().saturating_sub(1) as u64;
    recorded_run_at(dir, run, books, last, proposals);
}

fn recorded_run_at(
    dir: &std::path::Path,
    run: Digest,
    books: &[BookUpdate],
    market_through: u64,
    proposals: &[Proposal],
) {
    let mut ledger = StreamWriter::open(dir, RUNS).expect("ledger");
    record(
        &RunLedgerEntry {
            specification: run,
            class: "research".into(),
            started_ns: 1,
            ended_ns: 2,
            exit: ExitReason::Completed { status: 0 },
            emitted: Digest([9u8; 32]),
            strategy: Some(Digest([8u8; 32])),
            program: None,
            content: None,
        },
        &mut ledger,
    )
    .expect("record");

    let mut market = StreamWriter::open(dir, MARKET).expect("market");
    for b in books {
        market
            .append(&bijective_serialization::signal::market::encode(b).expect("encode"))
            .expect("append");
    }
    drop(market);
    let through = market_through;

    let mut stream = StreamWriter::open(dir, "proposals").expect("proposals");
    let from = stream.next_sequence();
    for p in proposals {
        stream
            .append(&bijective_serialization::signal::encode(p).expect("encode"))
            .expect("append");
    }
    let last = stream.next_sequence().saturating_sub(1);
    drop(stream);

    if !proposals.is_empty() {
        attribute(
            &Admitted {
                specification: run,
                from,
                through: last,
                count: proposals.len() as u64,
                market: MARKET.to_string(),
                market_through: through,
                proposals: "proposals".to_string(),
            },
            &mut ledger,
        )
        .expect("attribute");
    }
}

#[test]
fn a_crossing_proposal_is_priced_against_the_book_its_author_was_sent() {
    let dir = store("crossing");
    // 2 654 375 / 2 654 500: a spread of 125 on a mid of 2 654 437, which is 0.4708... bps.
    recorded_run(
        &dir,
        run_of(1),
        &[book("btc-perp", 2_654_375, 2_654_500)],
        &[proposal("btc-perp", Side::Buy, Some(2_654_500))],
    );

    let measured = measure(&dir, run_of(1)).expect("measure");

    assert_eq!(measured.observations, 1);
    assert_eq!(measured.placed.len(), 1);
    assert!(measured.placed[0].marketable, "a buy at the ask crosses");
    // 125 * 10 000 * 10 000 / 2 654 437 = 4 709.09..., rounded up against the strategy.
    assert_eq!(measured.placed[0].spread, 4_710, "the spread is in scaled basis points");
    assert_eq!(
        measured.measured,
        vec![
            Measured { measure: "spread_bps".into(), value: 4_710 },
            Measured { measure: "cost_floor_bps".into(), value: 9_420 },
        ],
        "a round trip crosses the spread twice"
    );
}

#[test]
fn a_limit_that_never_crossed_is_placed_but_not_counted() {
    let dir = store("resting");
    recorded_run(
        &dir,
        run_of(2),
        &[book("btc-perp", 2_654_375, 2_654_500)],
        &[proposal("btc-perp", Side::Buy, Some(2_600_000))],
    );

    let measured = measure(&dir, run_of(2)).expect("measure");

    assert_eq!(measured.placed.len(), 1, "it is still on the record");
    assert!(!measured.placed[0].marketable);
    assert_eq!(measured.observations, 0, "and it is not an observation");
    assert!(measured.measured.is_empty(), "nothing measured is not a figure of zero");
}

#[test]
fn a_market_order_crosses_and_is_counted() {
    let dir = store("market-order");
    recorded_run(
        &dir,
        run_of(3),
        &[book("btc-perp", 2_654_375, 2_654_500)],
        &[proposal("btc-perp", Side::Buy, None)],
    );

    assert_eq!(measure(&dir, run_of(3)).expect("measure").observations, 1);
}

#[test]
fn a_floor_missing_its_fee_is_unavailable_and_not_a_smaller_floor() {
    let dir = store("floor");
    recorded_run(
        &dir,
        run_of(4),
        &[book("btc-perp", 2_654_375, 2_654_500)],
        &[proposal("btc-perp", Side::Buy, Some(2_654_500))],
    );

    match measure(&dir, run_of(4)).expect("measure").floor {
        Floor::Unavailable { spread_round_trip, reason } => {
            assert_eq!(spread_round_trip, 9_420);
            assert!(reason.contains("fee"), "the reason names what is missing: {reason}");
        }
        other => panic!("an undeclared fee must not become a floor: {other:?}"),
    }
}

#[test]
fn a_proposal_for_another_instrument_is_refused_rather_than_priced() {
    let dir = store("other-instrument");
    recorded_run(
        &dir,
        run_of(5),
        &[book("btc-perp", 2_654_375, 2_654_500)],
        &[proposal("eth-perp", Side::Buy, Some(2_654_500))],
    );

    let measured = measure(&dir, run_of(5)).expect("measure");

    assert_eq!(measured.observations, 0);
    assert!(
        matches!(measured.unplaced.first(), Some(Unplaced::OtherInstrument { .. })),
        "{:?}",
        measured.unplaced
    );
}

#[test]
fn a_run_with_no_entry_cannot_be_measured() {
    let dir = store("absent");

    assert!(matches!(measure(&dir, run_of(6)), Err(MeasureError::NoSuchRun)));
}

#[test]
fn a_run_that_admitted_nothing_measures_nothing() {
    let dir = store("silent");
    recorded_run(&dir, run_of(7), &[book("btc-perp", 2_654_375, 2_654_500)], &[]);

    let measured = measure(&dir, run_of(7)).expect("measure");

    assert_eq!(measured.observations, 0);
    assert!(measured.placed.is_empty() && measured.unplaced.is_empty());
}

#[test]
fn an_interleaved_range_attributes_nothing() {
    let dir = store("interleaved");
    recorded_run(
        &dir,
        run_of(8),
        &[book("btc-perp", 2_654_375, 2_654_500)],
        &[proposal("btc-perp", Side::Buy, Some(2_654_500))],
    );
    let mut ledger = StreamWriter::open(&dir, RUNS).expect("ledger");
    attribute(
        &Admitted {
            specification: run_of(8),
            from: 0,
            through: 9,
            count: 2,
            market: MARKET.to_string(),
            market_through: 0,
            proposals: "proposals".to_string(),
        },
        &mut ledger,
    )
    .expect("attribute");
    drop(ledger);

    let measured = measure(&dir, run_of(8)).expect("measure");

    assert!(
        measured.unplaced.iter().any(|u| matches!(u, Unplaced::Interleaved { .. })),
        "{:?}",
        measured.unplaced
    );
}

#[test]
fn the_dispersion_is_the_spread_of_what_was_measured() {
    let dir = store("dispersion");
    recorded_run(
        &dir,
        run_of(9),
        &[book("btc-perp", 2_654_375, 2_654_500), book("btc-perp", 2_654_000, 2_655_000)],
        &[
            proposal("btc-perp", Side::Buy, Some(2_655_000)),
            proposal("btc-perp", Side::Buy, Some(2_655_000)),
        ],
    );

    let measured = measure(&dir, run_of(9)).expect("measure");

    assert_eq!(measured.observations, 2);
    assert_eq!(measured.dispersion, 0, "two identical figures have no spread between them");
}

#[test]
fn a_measurement_prices_against_the_stream_the_admission_names() {
    let dir = store("stream");
    recorded_run(
        &dir,
        run_of(10),
        &[book("btc-perp", 2_654_375, 2_654_500)],
        &[proposal("btc-perp", Side::Buy, Some(2_654_500))],
    );

    let measured = measure(&dir, run_of(10)).expect("measure");

    assert_eq!(measured.placed[0].market_position, 0);
}

fn registered(dir: &std::path::Path, spec: &RunSpec<Research>, horizon: Horizon) {
    registered_with(dir, spec, horizon, Vec::new());
}

fn registered_with(
    dir: &std::path::Path,
    spec: &RunSpec<Research>,
    horizon: Horizon,
    folds: Vec<crate::evidence::Fold>,
) {
    let registration = PreRegistration::declare(
        "the move over the horizon clears what it costs to take it",
        DecisionRule {
            measure: "net_after_spread_bps".into(),
            direction: Direction::AtLeast,
            threshold: 25,
        },
        horizon,
        folds,
        spec,
    )
    .expect("a hypothesis, a measure and a horizon");
    let mut registry = StreamWriter::open(dir, REGISTRATIONS).expect("registry");
    declare(&registration, 5, &mut registry).expect("declare");
}

fn declared_fee(dir: &std::path::Path, taker: i128) {
    let mut w = StreamWriter::open(dir, bijective_serialization::signal::fees::FEES).expect("fees");
    let payload = bijective_serialization::signal::fees::encode(
        &bijective_serialization::signal::fees::Fees {
            // `market-drill` is the stream, so `drill` is the venue behind it.
            venue: "drill".to_string(),
            taker,
            maker: -15_000,
            source: "a fixture, declared for this test".to_string(),
            read_at_ns: 1,
        },
    )
    .expect("encode");
    w.append(&payload).expect("append");
}

fn spec_of(bound: u64) -> RunSpec<Research> {
    RunSpec::<Research>::declare(
        "carry-v3",
        Vec::new(),
        std::collections::BTreeMap::new(),
        Some(bound),
        bijective_strategy_engine::runs::Exchange::Stepwise,
    )
    .expect("a research run with a declared position")
}

#[test]
fn a_return_is_marked_over_the_horizon_the_claim_declared() {
    let dir = store("horizon");
    let spec = spec_of(4_102);
    registered(&dir, &spec, Horizon::After { positions: 1 });
    recorded_run(
        &dir,
        spec.identity(),
        &[book("btc-perp", 2_654_375, 2_654_500), book("btc-perp", 2_659_375, 2_659_500)],
        &[proposal("btc-perp", Side::Buy, Some(3_000_000))],
    );

    let measured = measure(&dir, spec.identity()).expect("measure");

    assert!(
        matches!(measured.unplaced.first(), Some(Unplaced::HorizonBeyondTheRecord { .. })),
        "{:?}",
        measured.unplaced
    );
    assert_eq!(measured.observations, 0);
}

#[test]
fn a_return_over_a_horizon_the_record_holds_is_the_move_between_the_two_mids() {
    let dir = store("marked");
    let spec = spec_of(4_103);
    registered(&dir, &spec, Horizon::After { positions: 1 });
    // Three books: the admission names the second, and the horizon marks at the third.
    recorded_run_at(
        &dir,
        spec.identity(),
        &[
            book("btc-perp", 2_600_000, 2_600_100),
            book("btc-perp", 2_654_375, 2_654_500),
            book("btc-perp", 2_659_375, 2_659_500),
        ],
        1,
        &[proposal("btc-perp", Side::Buy, Some(3_000_000))],
    );

    let measured = measure(&dir, spec.identity()).expect("measure");

    assert_eq!(measured.observations, 1);
    // (2 659 437 - 2 654 437) * 10 000 * 10 000 / 2 654 437 = 188 363.06..., rounded down.
    assert_eq!(measured.placed[0].gross, Some(188_363));
    assert!(
        measured.measured.iter().any(|m| m.measure == "gross_return_bps" && m.value == 188_363),
        "{:?}",
        measured.measured
    );
    assert!(
        measured
            .measured
            .iter()
            .any(|m| m.measure == "net_after_spread_bps" && m.value == 188_363 - 9_420),
        "the net is the gross less the round trip: {:?}",
        measured.measured
    );
}

#[test]
fn a_sell_earns_when_the_mid_falls() {
    let dir = store("sell");
    let spec = spec_of(4_104);
    registered(&dir, &spec, Horizon::After { positions: 1 });
    recorded_run_at(
        &dir,
        spec.identity(),
        &[
            book("btc-perp", 2_600_000, 2_600_100),
            book("btc-perp", 2_659_375, 2_659_500),
            book("btc-perp", 2_654_375, 2_654_500),
        ],
        1,
        &[proposal("btc-perp", Side::Sell, Some(1))],
    );

    let measured = measure(&dir, spec.identity()).expect("measure");

    assert_eq!(measured.observations, 1, "a sell at 1 crosses a bid of 2 659 375");
    assert!(measured.placed[0].gross.expect("marked") > 0, "a fall is a gain for a sell");
}

#[test]
fn an_edge_that_fails_an_incomplete_floor_has_failed() {
    let dir = store("fails");
    let spec = spec_of(4_105);
    registered(&dir, &spec, Horizon::After { positions: 1 });
    recorded_run_at(
        &dir,
        spec.identity(),
        &[
            book("btc-perp", 2_600_000, 2_600_100),
            book("btc-perp", 2_654_375, 2_654_500),
            book("btc-perp", 2_654_375, 2_654_500),
        ],
        1,
        &[proposal("btc-perp", Side::Buy, Some(3_000_000))],
    );

    match measure(&dir, spec.identity()).expect("measure").clearance {
        Clearance::DoesNotClear { gross, floor } => {
            assert_eq!(gross, 0, "the mid did not move");
            assert_eq!(floor, 9_420);
        }
        other => panic!("a flat move cannot clear a spread: {other}"),
    }
}

#[test]
fn an_edge_that_clears_an_incomplete_floor_is_undecided_rather_than_cleared() {
    let dir = store("undecided");
    let spec = spec_of(4_106);
    registered(&dir, &spec, Horizon::After { positions: 1 });
    recorded_run_at(
        &dir,
        spec.identity(),
        &[
            book("btc-perp", 2_600_000, 2_600_100),
            book("btc-perp", 2_654_375, 2_654_500),
            book("btc-perp", 2_659_375, 2_659_500),
        ],
        1,
        &[proposal("btc-perp", Side::Buy, Some(3_000_000))],
    );

    match measure(&dir, spec.identity()).expect("measure").clearance {
        Clearance::Undecided { reason } => {
            assert!(reason.contains("fee"), "the reason names what is missing: {reason}");
        }
        other => panic!("a floor missing its fee cannot be cleared: {other}"),
    }
}

#[test]
fn a_claim_that_holds_nothing_has_no_return_to_compare() {
    let dir = store("not-held");
    let spec = spec_of(4_107);
    registered(&dir, &spec, Horizon::NotHeld);
    recorded_run(
        &dir,
        spec.identity(),
        &[book("btc-perp", 2_654_375, 2_654_500)],
        &[proposal("btc-perp", Side::Buy, Some(3_000_000))],
    );

    let measured = measure(&dir, spec.identity()).expect("measure");

    assert_eq!(measured.placed[0].gross, None, "no horizon is not a return of zero");
    assert!(!measured.measured.iter().any(|m| m.measure == "gross_return_bps"));
    match measured.clearance {
        Clearance::Undecided { reason } => {
            assert!(reason.contains("holds no position"), "{reason}")
        }
        other => panic!("nothing held cannot be compared: {other}"),
    }
}

#[test]
fn a_run_with_no_claim_reads_as_nobody_having_said() {
    let dir = store("unregistered-horizon");
    recorded_run(
        &dir,
        run_of(20),
        &[book("btc-perp", 2_654_375, 2_654_500)],
        &[proposal("btc-perp", Side::Buy, Some(3_000_000))],
    );

    let measured = measure(&dir, run_of(20)).expect("measure");

    assert_eq!(measured.horizon, Horizon::NotDeclared);
    match measured.clearance {
        Clearance::Undecided { reason } => {
            assert!(reason.contains("declares no horizon"), "{reason}")
        }
        other => panic!("{other}"),
    }
}

#[test]
fn a_declared_fee_completes_the_floor() {
    let dir = store("fee");
    let spec = spec_of(4_108);
    registered(&dir, &spec, Horizon::After { positions: 1 });
    declared_fee(&dir, 55_000);
    recorded_run_at(
        &dir,
        spec.identity(),
        &[
            book("btc-perp", 2_600_000, 2_600_100),
            book("btc-perp", 2_654_375, 2_654_500),
            book("btc-perp", 2_659_375, 2_659_500),
        ],
        1,
        &[proposal("btc-perp", Side::Buy, Some(3_000_000))],
    );

    let measured = measure(&dir, spec.identity()).expect("measure");

    // The spread round trip is 9 420 and the taker is taken twice.
    assert_eq!(measured.floor, Floor::Measured { round_trip: 9_420 + 110_000 });
    match measured.clearance {
        Clearance::Clears { gross, floor } => {
            assert_eq!(gross, 188_363);
            assert_eq!(floor, 119_420);
        }
        other => panic!("188 363 clears 119 420: {other}"),
    }
}

#[test]
fn a_fee_that_swallows_the_edge_turns_the_answer_over() {
    let dir = store("fee-heavy");
    let spec = spec_of(4_109);
    registered(&dir, &spec, Horizon::After { positions: 1 });
    declared_fee(&dir, 100_000);
    recorded_run_at(
        &dir,
        spec.identity(),
        &[
            book("btc-perp", 2_600_000, 2_600_100),
            book("btc-perp", 2_654_375, 2_654_500),
            book("btc-perp", 2_659_375, 2_659_500),
        ],
        1,
        &[proposal("btc-perp", Side::Buy, Some(3_000_000))],
    );

    match measure(&dir, spec.identity()).expect("measure").clearance {
        Clearance::DoesNotClear { gross, floor } => {
            assert_eq!(gross, 188_363);
            assert_eq!(floor, 9_420 + 200_000);
        }
        other => panic!("188 363 does not clear 209 420: {other}"),
    }
}

#[test]
fn an_undeclared_venue_is_named_in_the_reason() {
    let dir = store("no-fee");
    let spec = spec_of(4_110);
    registered(&dir, &spec, Horizon::NotHeld);
    recorded_run(
        &dir,
        spec.identity(),
        &[book("btc-perp", 2_654_375, 2_654_500)],
        &[proposal("btc-perp", Side::Buy, Some(3_000_000))],
    );

    match measure(&dir, spec.identity()).expect("measure").floor {
        Floor::Unavailable { reason, .. } => {
            assert!(reason.contains("drill"), "the reason names the venue: {reason}");
        }
        other => panic!("no fee is declared, so the floor cannot be complete: {other:?}"),
    }
}

#[test]
fn a_declared_structure_makes_the_figures_the_held_out_ones() {
    use crate::evidence::{Fold, Positions};

    let dir = store("held-out");
    let spec = spec_of(4_111);
    // Learn over the first two books, test over the third - which is the one the admission names.
    registered_with(
        &dir,
        &spec,
        Horizon::NotHeld,
        vec![Fold { learn: Positions::new(0, 2), test: Positions::new(2, 3) }],
    );
    recorded_run_at(
        &dir,
        spec.identity(),
        &[
            book("btc-perp", 2_600_000, 2_600_100),
            book("btc-perp", 2_650_000, 2_650_100),
            book("btc-perp", 2_654_375, 2_654_500),
        ],
        2,
        &[proposal("btc-perp", Side::Buy, Some(3_000_000))],
    );

    let measured = measure(&dir, spec.identity()).expect("measure");

    assert_eq!(measured.basis, Basis::HeldOut);
    assert_eq!(measured.observations, 1, "the proposal was decided inside the test window");
    assert_eq!(measured.folds.len(), 1);
    assert_eq!(measured.folds[0].observations, 1);
    assert_eq!(measured.folds[0].test, Positions::new(2, 3));
}

#[test]
fn a_proposal_inside_the_learn_window_is_not_one_of_the_held_out_figures() {
    use crate::evidence::{Fold, Positions};

    let dir = store("in-learn");
    let spec = spec_of(4_112);
    registered_with(
        &dir,
        &spec,
        Horizon::NotHeld,
        vec![Fold { learn: Positions::new(0, 2), test: Positions::new(2, 3) }],
    );
    // The admission names position 1, which is inside the learn window.
    recorded_run_at(
        &dir,
        spec.identity(),
        &[
            book("btc-perp", 2_600_000, 2_600_100),
            book("btc-perp", 2_650_000, 2_650_100),
            book("btc-perp", 2_654_375, 2_654_500),
        ],
        1,
        &[proposal("btc-perp", Side::Buy, Some(3_000_000))],
    );

    let measured = measure(&dir, spec.identity()).expect("measure");

    assert_eq!(measured.placed.len(), 1, "it is still on the record");
    assert_eq!(measured.observations, 0, "and it is not one of the figures the claim is judged on");
    assert_eq!(measured.folds[0].observations, 0);
    assert!(measured.measured.is_empty());
}

#[test]
fn a_claim_with_no_structure_is_measured_on_everything_and_says_so() {
    let dir = store("everything");
    let spec = spec_of(4_113);
    registered(&dir, &spec, Horizon::NotHeld);
    recorded_run(
        &dir,
        spec.identity(),
        &[book("btc-perp", 2_654_375, 2_654_500)],
        &[proposal("btc-perp", Side::Buy, Some(3_000_000))],
    );

    let measured = measure(&dir, spec.identity()).expect("measure");

    assert_eq!(measured.basis, Basis::Everything);
    assert_eq!(measured.observations, 1);
    assert!(measured.folds.is_empty());
}
