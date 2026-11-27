// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The table format: exact integers, absence kept apart from zero, and text that cannot break a
//! document or an event frame.

use bijective_limits::Ratio;

use super::*;
use crate::markup::money;

fn one_row(cells: Vec<Cell>, units: &[(&'static str, Unit)]) -> String {
    let mut table = Table::new(units);
    table.rows.push(cells);
    table.json("p", "P")
}

#[test]
fn integers_travel_as_strings_and_absence_as_null() {
    let units = [("at", Unit::Nanoseconds), ("gross", Unit::Money), ("net", Unit::Money)];
    let at = 1_763_900_000_123_456_789_i128;
    let json = one_row(vec![Cell::Integer(at), Cell::Integer(0), Cell::Absent], &units);
    assert_eq!(
        json,
        "{\"format\":1,\"panel\":\"p\",\"title\":\"P\",\"columns\":[\
         {\"name\":\"at\",\"unit\":\"ns\",\"scale\":null},\
         {\"name\":\"gross\",\"unit\":\"money\",\"scale\":8},\
         {\"name\":\"net\",\"unit\":\"money\",\"scale\":8}],\
         \"rows\":[[\"1763900000123456789\",\"0\",null]]}"
    );
}

#[test]
fn text_from_a_venue_cannot_break_the_document_or_the_frame() {
    let hostile = "a\"b\\c\nevent: x\rdata: y\u{1}\u{2028}";
    let json = one_row(
        vec![Cell::text(hostile), Cell::Flag(true)],
        &[("name", Unit::Text), ("open", Unit::Flag)],
    );
    assert!(!json.contains('\n') && !json.contains('\r') && !json.contains('\u{2028}'), "{json}");
    assert!(json.contains("[\"a\\\"b\\\\c\\nevent: x\\rdata: y\\u0001\\u2028\",true]"), "{json}");
}

/// The scales the format states are the ones the figures are recorded at.
#[test]
fn the_stated_scales_are_the_recorded_ones() {
    assert_eq!(money(10_i128.pow(u32::from(MONEY_SCALE))), "1.00");
    assert_eq!(Ratio::ONE, 10_i128.pow(u32::from(RATIO_SCALE)));
    assert_eq!(Unit::Instrument.scale(), None, "the row does not carry an instrument's scale");
}

#[test]
fn a_panel_is_addressed_by_its_title() {
    assert_eq!(slug("Orders"), "orders");
    assert_eq!(slug("Exposure by scope"), "exposure-by-scope");
    assert_eq!(slug("../x"), "---x");
}

#[test]
fn the_strip_is_a_table_of_its_signals() {
    let signals = [Signal { label: "node", value: "up".into(), level: Level::Alarm }];
    let json = strip(&signals).json("strip", "Strip");
    assert!(json.ends_with("\"rows\":[[\"node\",\"up\",\"alarm\"]]}"), "{json}");
}
