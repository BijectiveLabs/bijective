// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Acceptance gates for the C1 decision-capture core.
//! Gates (bounded buffer) and (latency budget) from execution quality attribution, plus capture-correctness rules.

use super::*;

const NS_PER_MS: i128 = 1_000_000;
const NS_PER_S: i128 = 1_000_000_000;
const T0: i128 = 1_800_000_000_000_000_000;

fn update(instrument: &str, observed_ns: i128) -> BookUpdate {
    BookUpdate {
        instrument: instrument.to_string(),
        best_bid_ticks: 9_990,
        best_bid_qty_lots: 50,
        best_ask_ticks: 10_010,
        best_ask_qty_lots: 50,
        observed_at: Instant::new(observed_ns, Resolution::Nanosecond),
    }
}

#[test]
fn buffer_never_grows_past_the_instrument_bound() {
    // A flood of distinct instruments, far exceeding the cap, must not grow the buffer.
    let mut buffer = SnapshotBuffer::new(4, NS_PER_S);
    for i in 0..1_000 {
        buffer.on_market_data(update(&format!("SYM-{i}"), T0), T0);
    }
    assert_eq!(buffer.len(), 4, "buffer grew past its instrument bound");
    assert_eq!(buffer.state(), BufferState::Degraded);
    assert!(buffer.degraded_events() > 0, "the bound was hit but not counted");
}

#[test]
fn updating_held_instruments_never_degrades() {
    // Updates to a held instrument cannot grow the buffer.
    let mut buffer = SnapshotBuffer::new(4, NS_PER_S);
    for sym in ["A", "B", "C", "D"] {
        buffer.on_market_data(update(sym, T0), T0);
    }
    for tick in 0..10_000 {
        buffer.on_market_data(update("A", T0 + tick), T0 + tick);
    }
    assert_eq!(buffer.len(), 4);
    assert_eq!(buffer.state(), BufferState::Healthy);
    assert_eq!(buffer.degraded_events(), 0);
}

#[test]
fn an_aged_out_instrument_is_evicted_freeing_room() {
    // Once an instrument ages past the bound it is dropped and its slot is reusable.
    let mut buffer = SnapshotBuffer::new(1, 100 * NS_PER_MS);
    buffer.on_market_data(update("OLD", T0), T0);
    assert_eq!(buffer.len(), 1);

    // The stale entry is evicted first, so the newcomer is admitted.
    let later = T0 + 200 * NS_PER_MS;
    buffer.on_market_data(update("NEW", later), later);
    assert_eq!(buffer.len(), 1);
    assert_eq!(buffer.state(), BufferState::Healthy);
    assert!(
        buffer.capture("OLD", "X", Instant::new(later, Resolution::Nanosecond), NS_PER_S).is_err()
    );
}

#[test]
fn capture_is_constant_work_within_a_generous_budget() {
    // Loose budget: it measures the algorithmic property, and a scan over a full buffer would blow through it.
    let mut buffer = SnapshotBuffer::new(10_000, NS_PER_S);
    for i in 0..10_000 {
        buffer.on_market_data(update(&format!("SYM-{i}"), T0), T0);
    }
    let decided = Instant::new(T0 + NS_PER_MS, Resolution::Nanosecond);

    let iterations = 1_000_000u32;
    let start = std::time::Instant::now();
    let mut captured = 0u64;
    for i in 0..iterations {
        let sym = format!("SYM-{}", i % 10_000);
        if buffer.capture(&sym, "X", decided, NS_PER_S).is_ok() {
            captured += 1;
        }
    }
    let elapsed = start.elapsed();

    assert_eq!(captured, iterations as u64, "some captures unexpectedly failed");
    // Loose ceiling so it does not flake. Any implementation that scans still fails.
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "capture latency budget exceeded: {elapsed:?} for {iterations} captures"
    );
}

#[test]
fn capture_records_the_lag_between_snapshot_and_decision() {
    let mut buffer = SnapshotBuffer::new(4, NS_PER_S);
    buffer.on_market_data(update("BTC-PERP", T0), T0);

    let decided = Instant::new(T0 + 40 * NS_PER_MS, Resolution::Nanosecond);
    let record = buffer
        .capture("BTC-PERP", "X-1", decided, NS_PER_S)
        .expect("a fresh snapshot should capture");

    assert_eq!(record.capture_lag_ns, 40 * NS_PER_MS);
    assert_eq!(record.decided_at, decided);
    assert_eq!(record.correlation_id, "X-1");
    assert_eq!(record.book.observed_at.value_ns, T0);
}

#[test]
fn capture_refuses_when_no_snapshot_is_held() {
    let buffer = SnapshotBuffer::new(4, NS_PER_S);
    let decided = Instant::new(T0, Resolution::Nanosecond);
    assert_eq!(
        buffer.capture("ABSENT", "X", decided, NS_PER_S),
        Err(CaptureError::NoSnapshotForInstrument)
    );
}

#[test]
fn capture_refuses_a_snapshot_older_than_the_stale_bound() {
    let mut buffer = SnapshotBuffer::new(4, NS_PER_S);
    buffer.on_market_data(update("BTC-PERP", T0), T0);

    let decided = Instant::new(T0 + NS_PER_S, Resolution::Nanosecond);
    let result = buffer.capture("BTC-PERP", "X", decided, 100 * NS_PER_MS);

    match result {
        Err(CaptureError::SnapshotStale { age_ns, bound_ns }) => {
            assert_eq!(age_ns, NS_PER_S);
            assert_eq!(bound_ns, 100 * NS_PER_MS);
        }
        other => panic!("expected a staleness refusal, got {other:?}"),
    }
}

#[test]
fn capture_refuses_a_snapshot_newer_than_the_decision() {
    // Snapshot stamped after the decision: refused, not a negative lag.
    let mut buffer = SnapshotBuffer::new(4, NS_PER_S);
    buffer.on_market_data(update("BTC-PERP", T0 + NS_PER_S), T0 + NS_PER_S);

    let decided = Instant::new(T0, Resolution::Nanosecond);
    assert_eq!(
        buffer.capture("BTC-PERP", "X", decided, NS_PER_S),
        Err(CaptureError::SnapshotAfterDecision)
    );
}

#[test]
fn capture_at_exactly_the_decision_instant_is_admissible() {
    // The same nanosecond is a zero lag, which is valid.
    let mut buffer = SnapshotBuffer::new(4, NS_PER_S);
    buffer.on_market_data(update("BTC-PERP", T0), T0);

    let decided = Instant::new(T0, Resolution::Nanosecond);
    let record = buffer.capture("BTC-PERP", "X", decided, NS_PER_S).expect("zero lag is valid");
    assert_eq!(record.capture_lag_ns, 0);
}

/// A benchmark missing while the buffer is refusing is different evidence.
/// Research integrity: an unwatched instrument is routine. A buffer that could not hold it is a gap in the record.
#[test]
fn a_missing_snapshot_says_so_differently_once_the_buffer_has_refused_one() {
    let mut buffer = SnapshotBuffer::new(1, NS_PER_S);
    buffer.on_market_data(update("AAA", T0), T0);

    // No room and nothing old enough to evict: refused and degraded.
    buffer.on_market_data(update("BBB", T0 + 1), T0 + 1);
    assert_eq!(buffer.state(), BufferState::Degraded);
    assert_eq!(buffer.degraded_events(), 1);

    match buffer.capture("BBB", "X", Instant::new(T0 + 2, Resolution::Nanosecond), NS_PER_S) {
        Err(CaptureError::BufferDegraded) => {}
        other => panic!("a refused admission read as a plain absence: {other:?}"),
    }
}

/// And a healthy buffer still reports a plain absence plainly.
#[test]
fn a_healthy_buffer_reports_an_unwatched_instrument_as_absent() {
    let buffer = SnapshotBuffer::new(4, NS_PER_S);
    assert_eq!(buffer.state(), BufferState::Healthy);

    match buffer.capture("AAA", "X", Instant::new(T0, Resolution::Nanosecond), NS_PER_S) {
        Err(CaptureError::NoSnapshotForInstrument) => {}
        other => panic!("{other:?}"),
    }
}

fn priced(observed_ns: i128, bid: i64) -> BookUpdate {
    BookUpdate { best_bid_ticks: bid, best_ask_ticks: bid + 20, ..update("A", observed_ns) }
}

/// A proposal is judged after newer books arrived: the benchmark is the book held at the
/// decision, never a later one and never none because a later one exists.
#[test]
fn the_benchmark_is_the_book_as_of_the_decision_with_newer_books_held() {
    let mut buffer = SnapshotBuffer::new(4, 10 * NS_PER_S);
    for (i, bid) in [100, 200, 300].into_iter().enumerate() {
        let at = T0 + i128::try_from(i).expect("small") * NS_PER_S;
        buffer.on_market_data(priced(at, bid), at);
    }
    let decided = Instant::new(T0 + NS_PER_S + 5, Resolution::Nanosecond);
    let record = buffer.capture("A", "c", decided, NS_PER_S).expect("captured");
    assert_eq!(record.book.best_bid_ticks, 200);
    assert_eq!(record.capture_lag_ns, 5);
    let before = Instant::new(T0 - 1, Resolution::Nanosecond);
    assert_eq!(
        buffer.capture("A", "c", before, NS_PER_S),
        Err(CaptureError::SnapshotAfterDecision)
    );
}

/// The history is capped whatever the feed's rate, oldest first out. A book observed before the
/// newest held is not kept, so the history stays in observed order.
#[test]
fn the_history_is_capped_and_kept_in_observed_order() {
    let mut buffer = SnapshotBuffer::new(1, 1_000 * NS_PER_S);
    let total = i128::try_from(HISTORY_PER_INSTRUMENT).expect("small") + 10;
    for i in 0..total {
        buffer.on_market_data(priced(T0 + i, 1), T0 + i);
    }
    let oldest = Instant::new(T0 + 9, Resolution::Nanosecond);
    assert_eq!(buffer.capture("A", "c", oldest, NS_PER_S), Err(CaptureError::SnapshotEvicted));
    assert!(buffer.capture("A", "c", Instant::new(T0 + 10, Resolution::Nanosecond), 1).is_ok());
    buffer.on_market_data(priced(T0, 7), T0 + total);
    let newest = Instant::new(T0 + total, Resolution::Nanosecond);
    let record = buffer.capture("A", "c", newest, NS_PER_S).expect("captured");
    assert_eq!(record.book.best_bid_ticks, 1, "the late book was not kept");
}
