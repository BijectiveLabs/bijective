// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The recorder's order and its replay, against a scripted feed so no venue format appears here.
//!
//! A frame `book:<price>` yields one snapshot record and a top. `bad` is refused by the feed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use bijective_archive::{ArchiveConfig, GapKind, all_book_rows, all_gap_rows};
use bijective_event_store::store::{StreamWriter, end_of, read_range};
use bijective_model::market::{BookLevel, BookSnapshot, ExchangeTime};
use bijective_model::{InstrumentId, Sequence};
use bijective_venue::contract::{
    BookTop, FeedError, MarketFact, MarketFeed, MarketRecord, Offered, VenueSymbol,
};
use bijective_venue::profile::{
    Instrument, InstrumentCatalogue, InstrumentKind, InstrumentStatus, PrecisionRules,
    PriceValidity,
};

use super::{Outputs, Recorder, RecorderError};
use crate::journal::{self, JournalEntry};

const VENUE: &str = "venue-a";
const T0: i128 = 1_793_664_000_000_000_000;

fn root(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("bl-recorder-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn catalogue() -> InstrumentCatalogue {
    InstrumentCatalogue::from_instruments([Instrument {
        symbol: VenueSymbol::new("AAA"),
        id: InstrumentId::new("venue-a.aaa"),
        kind: InstrumentKind::Perpetual,
        settlement_currency: "usdt".into(),
        quote_currency: "usdt".into(),
        base_currency: None,
        precision: PrecisionRules {
            price_scale: 2,
            quantity_scale: 3,
            price_tick: 1,
            quantity_lot: 1,
            price_validity: PriceValidity::IncrementOnly,
        },
        minimum_size: 1,
        minimum_notional: 1,
        status: InstrumentStatus::Trading,
    }])
}

#[derive(Default)]
struct ScriptedFeed {
    resets: u32,
}

impl MarketFeed for ScriptedFeed {
    fn offer(&mut self, frame: &[u8], received_at_ns: i128) -> Result<Offered, FeedError> {
        let text = std::str::from_utf8(frame).unwrap_or("");
        let Some(price) = text.strip_prefix("book:").and_then(|p| p.parse::<i128>().ok()) else {
            return Err(FeedError::Unreadable { detail: format!("{text} is not a book") });
        };
        let record = MarketRecord {
            instrument: VenueSymbol::new("AAA"),
            exchange_time: ExchangeTime::At { at_ns: received_at_ns - 1 },
            received_at_ns,
            fact: MarketFact::BookSnapshot(BookSnapshot {
                sequence: Sequence::NotNumbered,
                bids: vec![BookLevel { price_scaled: price, quantity_scaled: 1 }],
                asks: vec![BookLevel { price_scaled: price + 1, quantity_scaled: 1 }],
            }),
        };
        let top = BookTop {
            instrument: VenueSymbol::new("AAA"),
            sequence: Sequence::NotNumbered,
            venue_ts_ms: 1,
            bid_price: price,
            bid_quantity: 1,
            ask_price: price + 1,
            ask_quantity: 1,
            bid_depth: 1,
            ask_depth: 1,
        };
        Ok(Offered { records: vec![record], top: Some(top) })
    }

    fn reset(&mut self) {
        self.resets += 1;
    }

    fn accepted(&self) -> u64 {
        0
    }

    fn instruments(&self) -> usize {
        0
    }
}

fn archive_config(root: &Path) -> ArchiveConfig {
    ArchiveConfig {
        root: root.join("archive"),
        venue: VENUE.into(),
        price_scale: 2,
        quantity_scale: 3,
    }
}

fn entries() -> Vec<JournalEntry> {
    vec![
        JournalEntry::SessionStarted { at_ns: T0 },
        JournalEntry::Frame { received_at_ns: T0 + 1, frame: b"book:100".to_vec() },
        JournalEntry::Frame { received_at_ns: T0 + 2, frame: b"bad".to_vec() },
        JournalEntry::ContinuityLost { at_ns: T0 + 3 },
        JournalEntry::Frame { received_at_ns: T0 + 4, frame: b"book:101".to_vec() },
        JournalEntry::SessionEnded { at_ns: T0 + 5 },
    ]
}

/// Runs `entries` through a fresh recorder under `root`, archiving. Returns the feed's resets.
fn run(root: &Path, entries: &[JournalEntry]) -> u32 {
    let journal = StreamWriter::open(root, &journal::stream(VENUE)).unwrap();
    let mut market = StreamWriter::open(root, "market-venue-a").unwrap();
    let mut facts = StreamWriter::open(root, "facts-venue-a").unwrap();
    let catalogue = catalogue();
    let (mut recorder, _) = Recorder::new(
        journal,
        Outputs { market: &mut market, facts: &mut facts },
        &catalogue,
        Some((archive_config(root), T0)),
    )
    .unwrap();
    let mut feed = ScriptedFeed::default();
    for entry in entries {
        recorder.replay(entry, &mut feed).unwrap();
    }
    recorder.finish(T0 + 6).unwrap();
    feed.resets
}

/// Decision 0019: a top is held until its frame is durable. Before the commit a reader finds the
/// frame in the journal and no top on the market stream, after it finds both, in that order.
#[test]
fn a_top_is_written_only_after_its_frame_is_committed() {
    let r = root("held-top");
    let journal = StreamWriter::open(&r, &journal::stream(VENUE)).unwrap();
    let mut market = StreamWriter::open(&r, "market-venue-a").unwrap();
    let mut facts = StreamWriter::open(&r, "facts-venue-a").unwrap();
    let catalogue = catalogue();
    let outputs = Outputs { market: &mut market, facts: &mut facts };
    let (mut recorder, _) = Recorder::new(journal, outputs, &catalogue, None).unwrap();
    let mut feed = ScriptedFeed::default();

    let frame = JournalEntry::Frame { received_at_ns: T0 + 1, frame: b"book:100".to_vec() };
    recorder.replay(&frame, &mut feed).unwrap();
    assert_eq!(end_of(&r, &journal::stream(VENUE)).unwrap(), Some(0), "the frame is written");
    assert_eq!(end_of(&r, "market-venue-a").unwrap(), None, "its top waits for the commit");

    recorder.commit().unwrap();
    assert_eq!(end_of(&r, "market-venue-a").unwrap(), Some(0));
}

/// Many frames in one batch: every top reaches the market stream, in the order of its frames.
#[test]
fn one_commit_carries_a_whole_batch_in_order() {
    let r = root("batch-order");
    let journal = StreamWriter::open(&r, &journal::stream(VENUE)).unwrap();
    let mut market = StreamWriter::open(&r, "market-venue-a").unwrap();
    let mut facts = StreamWriter::open(&r, "facts-venue-a").unwrap();
    let catalogue = catalogue();
    let outputs = Outputs { market: &mut market, facts: &mut facts };
    let (mut recorder, _) = Recorder::new(journal, outputs, &catalogue, None).unwrap();
    let mut feed = ScriptedFeed::default();

    for price in 100..110 {
        let frame = JournalEntry::Frame {
            received_at_ns: T0 + i128::from(price),
            frame: format!("book:{price}").into_bytes(),
        };
        recorder.replay(&frame, &mut feed).unwrap();
    }
    recorder.commit().unwrap();
    let tops = stream(&r, "market-venue-a");
    assert_eq!(tops.len(), 10);
    let replayed: Vec<_> = (100..110)
        .map(|price| {
            let frame = format!("book:{price}").into_bytes();
            let top = feed.offer(&frame, T0).unwrap().top.unwrap();
            crate::encode(&super::from_top(&top, &catalogue, Some(T0 + i128::from(price))).unwrap())
                .unwrap()
        })
        .collect();
    assert_eq!(tops, replayed, "the tops are the batch's, in its order");
}

fn stream(root: &Path, name: &str) -> Vec<Vec<u8>> {
    let last = end_of(root, name).unwrap().expect("written");
    read_range(root, name, 0, last).unwrap().into_iter().map(|e| e.payload).collect()
}

#[test]
fn every_entry_is_journaled_as_it_was_given() {
    let r = root("journal");
    run(&r, &entries());
    let journaled: Vec<JournalEntry> =
        stream(&r, &journal::stream(VENUE)).iter().map(|p| journal::decode(p).unwrap()).collect();
    assert_eq!(journaled, entries());
}

/// The archive row names the journal entry its frame was written at, so it traces to the bytes.
#[test]
fn an_archived_row_names_the_journal_entry_of_its_frame() {
    let r = root("trace");
    run(&r, &entries());
    let rows = all_book_rows(&archive_config(&r)).unwrap();
    let seqs: Vec<u64> = rows.iter().map(|row| row.journal_seq).collect();
    assert_eq!(seqs, vec![1, 1, 4, 4], "two levels each from the frames at entries 1 and 4");
}

#[test]
fn a_refused_frame_is_journaled_and_marked_and_books_drop_where_the_journal_says() {
    let r = root("gaps");
    let resets = run(&r, &entries());
    assert_eq!(resets, 1);
    let gaps: Vec<(u64, i64, GapKind)> = all_gap_rows(&archive_config(&r))
        .unwrap()
        .into_iter()
        .map(|g| (g.journal_seq, g.at_ns, g.kind))
        .collect();
    let at = |offset: i128| (T0 + offset) as i64;
    assert_eq!(
        gaps,
        vec![
            (0, at(0), GapKind::SessionStarted),
            (2, at(2), GapKind::FrameRefused),
            (3, at(3), GapKind::ContinuityLost),
            (5, at(5), GapKind::SessionEnded),
        ]
    );
}

/// The parity the journal exists for: replaying it gives the same journal, market stream and
/// archive files, byte for byte.
#[test]
fn replaying_a_journal_reproduces_every_output_byte_for_byte() {
    let (a, b) = (root("first"), root("again"));
    run(&a, &entries());
    let replayed: Vec<JournalEntry> =
        stream(&a, &journal::stream(VENUE)).iter().map(|p| journal::decode(p).unwrap()).collect();
    run(&b, &replayed);

    assert_eq!(stream(&a, &journal::stream(VENUE)), stream(&b, &journal::stream(VENUE)));
    assert_eq!(stream(&a, "market-venue-a"), stream(&b, "market-venue-a"));
    let files = |r: &Path| -> Vec<Vec<u8>> {
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut pending = vec![archive_config(r).venue_dir()];
        while let Some(d) = pending.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    pending.push(p);
                } else if p.extension().is_some_and(|x| x == "parquet") {
                    paths.push(p);
                }
            }
        }
        paths.sort();
        paths.iter().map(|p| std::fs::read(p).unwrap()).collect()
    };
    let (fa, fb) = (files(&a), files(&b));
    assert_eq!(fa.len(), 2, "one book file and one gaps file");
    assert_eq!(fa, fb);
}

#[test]
fn a_record_with_no_frame_before_it_is_refused() {
    let r = root("no-frame");
    let journal = StreamWriter::open(&r, &journal::stream(VENUE)).unwrap();
    let mut market = StreamWriter::open(&r, "market-venue-a").unwrap();
    let mut facts = StreamWriter::open(&r, "facts-venue-a").unwrap();
    let catalogue = catalogue();
    let (mut recorder, _) = Recorder::new(
        journal,
        Outputs { market: &mut market, facts: &mut facts },
        &catalogue,
        None,
    )
    .unwrap();
    let offered = ScriptedFeed::default().offer(b"book:1", T0).unwrap();
    assert!(matches!(recorder.record(&offered.records[0]), Err(RecorderError::NoFrame)));
}

#[test]
fn without_an_archive_records_are_counted_and_an_undeclared_symbol_is_refused() {
    let r = root("no-archive");
    let journal = StreamWriter::open(&r, &journal::stream(VENUE)).unwrap();
    let mut market = StreamWriter::open(&r, "market-venue-a").unwrap();
    let mut facts = StreamWriter::open(&r, "facts-venue-a").unwrap();
    let empty = InstrumentCatalogue::from_instruments([]);
    let (mut recorder, report) =
        Recorder::new(journal, Outputs { market: &mut market, facts: &mut facts }, &empty, None)
            .unwrap();
    assert!(report.is_none());
    let frame = JournalEntry::Frame { received_at_ns: T0, frame: b"book:1".to_vec() };
    let refused = recorder.replay(&frame, &mut ScriptedFeed::default());
    assert!(matches!(refused, Err(RecorderError::NotInCatalogue(_))));
    assert_eq!((recorder.counts().frames, recorder.counts().records), (1, 1));
}

/// A trade reaching a recorder with an archive is archived as a fact and recording goes on. It
/// once stopped the program: the archive had no table for it and refused it.
#[test]
fn a_trade_reaches_the_archive_and_recording_goes_on() {
    use bijective_model::VenueSymbol;
    use bijective_model::market::{Aggressor, ExchangeTime, MarketFact, MarketRecord, Trade};

    let r = root("no-table");
    let journal = StreamWriter::open(&r, &journal::stream(VENUE)).unwrap();
    let mut market = StreamWriter::open(&r, "market-venue-a").unwrap();
    let mut facts = StreamWriter::open(&r, "facts-venue-a").unwrap();
    let catalogue = catalogue();
    let (mut recorder, _) = Recorder::new(
        journal,
        Outputs { market: &mut market, facts: &mut facts },
        &catalogue,
        Some((archive_config(&r), T0)),
    )
    .unwrap();
    recorder.frame(T0 + 1, b"trade").unwrap();
    let trade = MarketRecord {
        instrument: VenueSymbol::new("AAA"),
        exchange_time: ExchangeTime::NotPublished,
        received_at_ns: T0 + 1,
        fact: MarketFact::Trade(Trade {
            trade_id: "t-1".into(),
            aggressor: Aggressor::Buyer,
            price_scaled: 100,
            quantity_scaled: 1,
        }),
    };
    recorder.record(&trade).expect("a trade does not stop the recorder");
    let counts = recorder.archive_counts().expect("archiving");
    assert_eq!((counts.not_archived, counts.buffered_rows), (0, 1));
    recorder.finish(T0 + 2).unwrap();
    assert_eq!(bijective_archive::all_fact_rows(&archive_config(&r)).unwrap().len(), 1);

    let written = stream(&r, "facts-venue-a");
    assert_eq!(written.len(), 1, "the trade is also on the facts stream strategies read");
    let read = bijective_serialization::signal::facts::decode(&written[0]).expect("a fact");
    assert_eq!(read.instrument, InstrumentId::new("venue-a.aaa"));
    assert_eq!((read.received_at_ns, read.fact), (T0 + 1, trade.fact));
}

/// A fact on a symbol the catalogue does not declare has no platform identity to be sent under:
/// it stays in the journal, is counted, and stops nothing.
#[test]
fn a_fact_on_an_undeclared_symbol_is_counted_not_sent() {
    use bijective_model::market::{MarketFact, MarketRecord};

    let r = root("undeclared-fact");
    let journal = StreamWriter::open(&r, &journal::stream(VENUE)).unwrap();
    let mut market = StreamWriter::open(&r, "market-venue-a").unwrap();
    let mut facts = StreamWriter::open(&r, "facts-venue-a").unwrap();
    let catalogue = catalogue();
    let outputs = Outputs { market: &mut market, facts: &mut facts };
    let (mut recorder, _) = Recorder::new(journal, outputs, &catalogue, None).unwrap();
    recorder.frame(T0 + 1, b"mark").unwrap();
    let mark = MarketRecord {
        instrument: VenueSymbol::new("ZZZ"),
        exchange_time: ExchangeTime::NotPublished,
        received_at_ns: T0 + 1,
        fact: MarketFact::MarkPrice { mark_price_scaled: 5 },
    };
    recorder.record(&mark).expect("an undeclared fact does not stop the recorder");
    assert_eq!((recorder.counts().facts, recorder.counts().undeclared_facts), (0, 1));
    assert_eq!(end_of(&r, "facts-venue-a").unwrap(), None);
}
