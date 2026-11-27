// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;
use bijective_venue::profile::{
    Instrument, InstrumentId, InstrumentKind, InstrumentStatus, PrecisionRules, PriceValidity,
};

fn catalogue() -> InstrumentCatalogue {
    InstrumentCatalogue::from_instruments(vec![Instrument {
        symbol: VenueSymbol::new("ABCUSDT"),
        id: InstrumentId::new("venue.ABCUSDT"),
        kind: InstrumentKind::Perpetual,
        settlement_currency: "usdt".into(),
        quote_currency: "usdt".into(),
        base_currency: Some("abc".into()),
        precision: PrecisionRules {
            price_scale: 2,
            quantity_scale: 3,
            price_tick: 10,
            quantity_lot: 1,
            price_validity: PriceValidity::IncrementOnly,
        },
        minimum_size: 1,
        minimum_notional: 0,
        status: InstrumentStatus::Trading,
    }])
}

fn reading(format: Format, catalogue: &InstrumentCatalogue) -> Reading<'_> {
    Reading {
        format,
        unit: TimeUnit::Milliseconds,
        catalogue,
        renamed: BTreeMap::new(),
        instrument: None,
    }
}

const HEADER: &str = "time,instrument,bid_price,bid_quantity,ask_price,ask_quantity";

#[test]
fn a_csv_row_is_a_top_of_book_at_the_instruments_precision() {
    let catalogue = catalogue();
    let lines = [HEADER, "1800000000000,ABCUSDT,26543.70,1.5,26545.00,0.9"];

    let books = read_books(lines.into_iter(), &reading(Format::Csv, &catalogue)).expect("read");

    assert_eq!(books.len(), 1);
    let book = &books[0];
    assert_eq!(book.instrument, InstrumentId::new("venue.ABCUSDT"));
    assert_eq!((book.bid_price, book.ask_price), (2_654_370, 2_654_500));
    assert_eq!((book.bid_quantity, book.ask_quantity), (1_500, 900));
    assert_eq!(book.venue_ts_ms, 1_800_000_000_000);
    assert_eq!(book.seq, Sequence::NotNumbered, "a file numbers nothing the venue published");
}

#[test]
fn a_json_line_reads_the_same_as_its_csv_row() {
    let catalogue = catalogue();
    let csv = [HEADER, "1800000000000,ABCUSDT,26543.70,1.5,26545.00,0.9"];
    let json = [
        r#"{"time":1800000000000,"instrument":"ABCUSDT","bid_price":"26543.70","bid_quantity":"1.5","ask_price":"26545.00","ask_quantity":"0.9"}"#,
    ];

    let from_csv = read_books(csv.into_iter(), &reading(Format::Csv, &catalogue));
    let from_json = read_books(json.into_iter(), &reading(Format::JsonLines, &catalogue));
    assert_eq!(from_csv, from_json);
}

#[test]
fn a_value_finer_than_the_instrument_is_refused_with_its_line() {
    let catalogue = catalogue();
    let lines = [HEADER, "1,ABCUSDT,1.00,1,2.00,1", "2,ABCUSDT,1.005,1,2.00,1"];

    let refused = read_books(lines.into_iter(), &reading(Format::Csv, &catalogue));
    let refused = refused.expect_err("excess precision is refused, never rounded");
    assert_eq!(refused.line, 3);
    assert!(refused.to_string().contains("bid_price"), "{refused}");
}

#[test]
fn time_that_goes_back_is_refused() {
    let catalogue = catalogue();
    let lines = [HEADER, "5,ABCUSDT,1.00,1,2.00,1", "4,ABCUSDT,1.00,1,2.00,1"];

    let refused = read_books(lines.into_iter(), &reading(Format::Csv, &catalogue));
    assert!(matches!(refused, Err(Refused { line: 3, error: RowError::Backwards { .. } })));
}

#[test]
fn quotes_floats_and_unlisted_instruments_are_refused() {
    let catalogue = catalogue();
    let quoted = [HEADER, "1,\"ABCUSDT\",1.00,1,2.00,1"];
    let float = [
        r#"{"time":1,"instrument":"ABCUSDT","bid_price":1.5,"bid_quantity":"1","ask_price":"2","ask_quantity":"1"}"#,
    ];
    let unlisted = [HEADER, "1,XYZUSDT,1.00,1,2.00,1"];

    let csv = reading(Format::Csv, &catalogue);
    let json = reading(Format::JsonLines, &catalogue);
    assert!(matches!(
        read_books(quoted.into_iter(), &csv),
        Err(Refused { error: RowError::Quoted, .. })
    ));
    assert!(matches!(
        read_books(float.into_iter(), &json),
        Err(Refused { error: RowError::Float { .. }, .. })
    ));
    assert!(matches!(
        read_books(unlisted.into_iter(), &csv),
        Err(Refused { error: RowError::Unlisted { .. }, .. })
    ));
}

#[test]
fn renamed_columns_and_a_fixed_instrument_are_read() {
    let catalogue = catalogue();
    let mut read = reading(Format::Csv, &catalogue);
    read.unit = TimeUnit::Seconds;
    read.instrument = Some(VenueSymbol::new("ABCUSDT"));
    read.renamed = [("time", "ts"), ("bid_price", "bid"), ("ask_price", "ask")]
        .into_iter()
        .map(|(c, f)| (c.to_string(), f.to_string()))
        .collect();
    let lines = ["ts,bid,bid_quantity,ask,ask_quantity", "1800000000,1.00,1,2.00,1"];

    let books = read_books(lines.into_iter(), &read).expect("read");
    assert_eq!(books[0].venue_ts_ms, 1_800_000_000_000);
    assert_eq!(books[0].bid_price, 100);
}
