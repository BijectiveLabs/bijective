// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The facts table: trades, liquidations, funding, open interest, mark and index prices.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use bijective_model::market::{
    Aggressor, ExchangeTime, Funding, Liquidation, MarketFact, MarketRecord, OpenInterest,
    ScaledDecimal, Trade,
};
use bijective_model::{Side, VenueSymbol};

use super::*;
use crate::{ArchiveConfig, ArchiveWriter, all_fact_rows, compact, newest_fact_rows, verify};

/// 2026-11-03 00:00:00 UTC.
const DAY: i128 = 1_793_664_000_000_000_000;

fn root(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("bl-facts-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn config(root: &Path) -> ArchiveConfig {
    ArchiveConfig {
        root: root.to_path_buf(),
        venue: "venue-a".into(),
        price_scale: 2,
        quantity_scale: 3,
    }
}

fn record(at: i128, fact: MarketFact) -> MarketRecord {
    MarketRecord {
        instrument: VenueSymbol::new("BTCUSDT"),
        exchange_time: ExchangeTime::At { at_ns: at - 1_000 },
        received_at_ns: at,
        fact,
    }
}

fn facts() -> Vec<MarketRecord> {
    vec![
        record(
            DAY + 1,
            MarketFact::Trade(Trade {
                trade_id: "t-1".into(),
                aggressor: Aggressor::Seller,
                price_scaled: 1_000_050,
                quantity_scaled: 7,
            }),
        ),
        record(
            DAY + 2,
            MarketFact::Liquidation(Liquidation {
                forced_order_side: Side::Buy,
                price_scaled: 999_900,
                quantity_scaled: 2_000,
            }),
        ),
        record(
            DAY + 3,
            MarketFact::Funding(Funding {
                rate: ScaledDecimal { value_scaled: -5_671, scale: 8 },
                next_funding_time: ExchangeTime::At { at_ns: DAY + 8 * 3_600_000_000_000 },
            }),
        ),
        record(
            DAY + 4,
            MarketFact::OpenInterest(OpenInterest::BaseQuantity { quantity_scaled: 492_373_720 }),
        ),
        record(
            DAY + 5,
            MarketFact::OpenInterest(OpenInterest::QuoteValue {
                value: ScaledDecimal { value_scaled: 332_488, scale: 2 },
            }),
        ),
        record(DAY + 6, MarketFact::MarkPrice { mark_price_scaled: 6_666_660 }),
        record(DAY + 7, MarketFact::IndexPrice { index_price_scaled: 6_667_000 }),
    ]
}

fn session(root: &Path, records: &[MarketRecord]) -> crate::ArchiveCounts {
    let (mut w, _) = ArchiveWriter::open(config(root), DAY).expect("open");
    for (seq, r) in records.iter().enumerate() {
        w.append_record(r, seq as u64 + 1, 0).expect("append");
    }
    w.close(DAY + 60_000_000_000).expect("close")
}

#[test]
fn every_kind_the_adapters_produce_reads_back_as_it_was_written() {
    let r = root("roundtrip");
    let counts = session(&r, &facts());
    assert_eq!(counts.not_archived, 0, "every kind produced has a table");

    let expected: Vec<FactRow> = facts()
        .iter()
        .enumerate()
        .map(|(seq, rec)| fact_row(rec, seq as u64 + 1, 0).expect("a fact").expect("a row"))
        .collect();
    assert_eq!(all_fact_rows(&config(&r)).expect("read"), expected);

    let kinds: Vec<FactKind> = expected.iter().map(|f| f.kind).collect();
    assert_eq!(
        kinds,
        [
            FactKind::Trade,
            FactKind::Liquidation,
            FactKind::Funding,
            FactKind::OpenInterest,
            FactKind::OpenInterestValue,
            FactKind::Mark,
            FactKind::Index,
        ]
    );
    assert_eq!(expected[0].side, Some(FactSide::Seller));
    assert_eq!(expected[0].trade_id.as_deref(), Some("t-1"));
    assert_eq!((expected[2].value_scaled, expected[2].value_scale), (Some(-5_671), Some(8)));
    assert!(verify(&config(&r)).expect("verify").is_sound());
}

#[test]
fn the_same_facts_produce_the_same_bytes() {
    let (a, b) = (root("det-a"), root("det-b"));
    session(&a, &facts());
    session(&b, &facts());
    let bytes = |r: &Path| {
        let dir = config(r).venue_dir().join("facts");
        crate::writer::files_under(&dir)
            .unwrap()
            .into_iter()
            .map(|p| std::fs::read(p).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(bytes(&a).len(), 1);
    assert_eq!(bytes(&a), bytes(&b));
}

/// A kind no adapter produces yet is counted, never fatal.
#[test]
fn a_kind_with_no_columns_yet_is_counted() {
    let ratio = record(
        DAY + 1,
        MarketFact::LongShortRatio(bijective_model::market::LongShortRatio {
            long_share: ScaledDecimal { value_scaled: 55, scale: 2 },
            short_share: ScaledDecimal { value_scaled: 45, scale: 2 },
            period_ns: 300_000_000_000,
        }),
    );
    assert_eq!(fact_row(&ratio, 1, 0).expect("not refused"), None);
    let counts = session(&root("ratio"), &[ratio]);
    assert_eq!((counts.not_archived, counts.sealed_rows), (1, 0));
}

#[test]
fn a_facts_partition_compacts_and_reads_the_same() {
    let r = root("compact");
    let mut later = facts();
    for (n, rec) in later.iter_mut().enumerate() {
        rec.received_at_ns += 60_000_000_000 + n as i128;
    }
    let mut all = facts();
    all.extend(later);
    session(&r, &all);
    let before = all_fact_rows(&config(&r)).unwrap();
    let done = compact(&config(&r), "facts", "2026-11-03").expect("compacted");
    assert_eq!(done.inputs, 2);
    assert_eq!(all_fact_rows(&config(&r)).unwrap(), before);
}

/// Three windows: two on the first day, one on the next, seven facts each.
fn three_windows() -> Vec<MarketRecord> {
    let mut all = Vec::new();
    for offset in [0, 60_000_000_000, 86_400_000_000_000] {
        let mut window = facts();
        for rec in &mut window {
            rec.received_at_ns += offset;
        }
        all.extend(window);
    }
    all
}

/// A compaction lists the first day after the second. The newest file is still the second day's.
#[test]
fn the_newest_facts_are_by_time_not_by_manifest_order_and_carry_the_declared_scales() {
    let r = root("newest");
    session(&r, &three_windows());
    compact(&config(&r), "facts", "2026-11-03").expect("compacted");

    let newest = newest_fact_rows(&r, "venue-a", 7).expect("read").expect("facts");
    assert_eq!((newest.price_scale, newest.quantity_scale), (2, 3));
    let seqs: Vec<u64> = newest.rows.iter().map(|f| f.journal_seq).collect();
    assert_eq!(seqs, (15..=21).collect::<Vec<u64>>(), "only the second day's file fits");

    let all = newest_fact_rows(&r, "venue-a", 21).expect("read").expect("facts");
    let seqs: Vec<u64> = all.rows.iter().map(|f| f.journal_seq).collect();
    assert_eq!(seqs, (1..=21).collect::<Vec<u64>>(), "in journal order");
}

#[test]
fn the_newest_file_is_read_whatever_the_budget_and_a_venue_with_none_is_none() {
    let r = root("budget");
    session(&r, &facts());
    let newest = newest_fact_rows(&r, "venue-a", 0).expect("read").expect("facts");
    assert_eq!(newest.rows.len(), 7);
    assert_eq!(newest_fact_rows(&r, "venue-b", 100).expect("read"), None);
}

/// A resolution is a row of its own kind: 1 for the outcome that won, 0 for one that lost.
#[test]
fn a_resolution_reads_back_as_won_or_lost() {
    let r = root("resolution");
    let written = [
        record(DAY + 1, MarketFact::Resolution { won: true }),
        record(DAY + 2, MarketFact::Resolution { won: false }),
    ];
    let counts = session(&r, &written);
    assert_eq!(counts.not_archived, 0);

    let rows = all_fact_rows(&config(&r)).expect("read");
    let read: Vec<(FactKind, Option<i64>, Option<u8>)> =
        rows.iter().map(|f| (f.kind, f.value_scaled, f.value_scale)).collect();
    assert_eq!(
        read,
        [(FactKind::Resolution, Some(1), Some(0)), (FactKind::Resolution, Some(0), Some(0))]
    );
}
