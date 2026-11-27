// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("bijective-fees-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch");
    d
}

fn fees(venue: &str, taker: i128, maker: i128) -> Fees {
    Fees {
        venue: venue.to_string(),
        taker,
        maker,
        source: "the venue's published schedule, read on 2026-09-02".to_string(),
        read_at_ns: 1_788_000_000_000_000_000,
    }
}

#[test]
fn a_declaration_reads_back_as_what_was_written() {
    let declared = fees("drill", 55_000, -15_000);

    assert_eq!(decode(&encode(&declared).expect("encode")).expect("decode"), declared);
}

#[test]
fn a_maker_rebate_survives_as_a_negative_figure() {
    let read = decode(&encode(&fees("drill", 55_000, -15_000)).expect("encode")).expect("decode");

    assert_eq!(read.maker, -15_000);
}

/// A figure that cannot be verified is not a declaration.
#[test]
fn a_fee_with_no_source_is_refused() {
    let mut nothing = fees("drill", 55_000, 0);
    nothing.source = "   ".to_string();

    assert!(matches!(encode(&nothing), Err(FeesError::NoSource)));
}

/// The latest stands, and the earlier one stays where it was.
#[test]
fn the_latest_declaration_is_the_one_that_stands() {
    let root = dir("standing");
    let mut w = bijective_event_store::store::StreamWriter::open(&root, FEES).expect("open");
    w.append(&encode(&fees("drill", 55_000, 0)).expect("encode")).expect("append");
    w.append(&encode(&fees("other", 10_000, 0)).expect("encode")).expect("append");
    w.append(&encode(&fees("drill", 60_000, 0)).expect("encode")).expect("append");
    drop(w);

    assert_eq!(standing(&root, "drill").expect("read").expect("declared").taker, 60_000);
    assert_eq!(standing(&root, "other").expect("read").expect("declared").taker, 10_000);
    assert!(standing(&root, "absent").expect("read").is_none(), "an undeclared venue has none");
}
