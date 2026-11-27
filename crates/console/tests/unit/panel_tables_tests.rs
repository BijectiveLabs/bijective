// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Every panel answers as a table from the same fold that renders it.

use bijective_control_engine::wire::{ControlEvent, Valued};
use bijective_lifecycle::{AccountCharge, Fee, Fill, Liquidity};
use bijective_limits::{Amount, LimitKind, Scope, Threshold};
use bijective_model::market::ExchangeTime;
use bijective_model::{OrderRequest, Side, VenueSymbol};
use bijective_risk::limit_state::wire::LimitEvent;

use super::*;
use crate::panel::{Entry, Panel};
use crate::table::Cell;

fn intent() -> ControlEvent {
    ControlEvent::Intent {
        decision_id: "d-1".into(),
        venue: Some("venue-a".into()),
        owner: None,
        request: OrderRequest {
            client_id: "bl-1".into(),
            instrument: VenueSymbol::new("aaa"),
            side: Side::Buy,
            quantity_scaled: 1_000,
            price_scaled: Some(200),
            reduce_only: false,
            terms: Default::default(),
        },
        from_position: 0,
        at_ns: 10,
    }
}

fn fee(amount_scaled: i128) -> Fee {
    Fee { asset: "USDC".into(), amount_scaled, scale: 4 }
}

fn filled() -> ControlEvent {
    let fill = Fill {
        fill_id: "t-1".into(),
        venue_order_id: Some("v-1".into()),
        side: Side::Buy,
        price_scaled: 200,
        quantity_scaled: 400,
        fee: Some(fee(125)),
        liquidity: Liquidity::Maker,
        exchange_time: ExchangeTime::At { at_ns: 15 },
        received_at_ns: 16,
    };
    ControlEvent::Filled { client_id: "bl-1".into(), fill, at_ns: 17 }
}

fn charged() -> ControlEvent {
    let charge = AccountCharge { charge_id: "funding:aaa:1".into(), fee: fee(-30) };
    ControlEvent::AccountCharged { venue: "venue-a".into(), charge, at_ns: 18 }
}

fn valued(gross: i128, at_ns: i128) -> ControlEvent {
    let figures = Valued {
        gross_scaled: Some(gross),
        net_scaled: Some(gross),
        daily_loss_scaled: None,
        observed_at_ns: at_ns - 1,
        leverage_scaled: None,
        margin_use_scaled: None,
        equity_scaled: None,
    };
    ControlEvent::PortfolioValued { figures, at_ns }
}

fn gross_limit(maximum: i128) -> LimitEvent {
    LimitEvent::Declared {
        scope: Scope::Portfolio,
        kind: LimitKind::GrossExposure,
        maximum: Threshold::Money(Amount::from_scaled(maximum)),
        config_version: 1,
    }
}

fn fold(panel: &mut dyn Panel) {
    for event in [intent(), filled(), charged(), valued(5, 20), valued(6, 30)] {
        panel.apply(&Entry::Control(&event));
    }
    panel.apply(&Entry::Limit(&gross_limit(100)));
}

/// The control forms are the one page that is not a table: they issue, they do not report.
#[test]
fn every_panel_but_the_control_forms_answers_as_a_table_with_one_cell_per_column() {
    for group in groups() {
        for mut panel in group.panels {
            fold(panel.as_mut());
            let Some(table) = panel.rows() else {
                assert_eq!(panel.title(), "Control", "{} answers no table", panel.title());
                continue;
            };
            for row in &table.rows {
                assert_eq!(row.len(), table.columns.len(), "{}", panel.title());
            }
        }
    }
}

#[test]
fn a_fill_carries_its_order_venue_and_its_fee_as_the_exact_decimal() {
    let mut fills = FillsPanel::default();
    fold(&mut fills);
    let table = fills.rows().expect("a table");
    assert_eq!(table.rows.len(), 1);
    let row = &table.rows[0];
    assert_eq!(row[2], Cell::text("venue-a"));
    assert_eq!(row[4], Cell::text("aaa"));
    assert_eq!(row[8], Cell::text("0.0125"));
    assert_eq!(row[10], Cell::text("Maker"));
}

#[test]
fn a_fill_whose_intent_was_not_read_names_no_venue_rather_than_a_guess() {
    let mut fills = FillsPanel::default();
    fills.apply(&Entry::Control(&filled()));
    let row = &fills.rows().expect("a table").rows[0];
    assert_eq!((&row[2], &row[4]), (&Cell::Absent, &Cell::Absent));
}

#[test]
fn a_charge_keeps_its_sign_received_is_negative() {
    let mut charges = ChargesPanel::default();
    fold(&mut charges);
    let row = &charges.rows().expect("a table").rows[0];
    assert_eq!((&row[1], &row[2]), (&Cell::text("funding:aaa:1"), &Cell::text("-0.0030")));
}

/// The history is the record's, not a reader's: every kept record is a row, and an older row
/// keeps the limit of its own time.
#[test]
fn the_portfolio_answers_every_kept_record_each_against_the_limit_of_its_time() {
    let mut exposure = ExposurePanel::default();
    exposure.apply(&Entry::Control(&valued(5, 20)));
    exposure.apply(&Entry::Limit(&gross_limit(100)));
    exposure.apply(&Entry::Control(&valued(6, 30)));
    exposure.apply(&Entry::Limit(&gross_limit(200)));
    let rows = exposure.rows().expect("a table").rows;
    assert_eq!(rows.len(), 2);
    assert_eq!((&rows[0][0], &rows[0][3]), (&Cell::Integer(20), &Cell::Absent));
    assert_eq!(rows[1][3], Cell::Integer(200), "the newest is judged as the page judges it");
}

fn detection(level: crate::detection::Raised) -> crate::detection::Detection {
    crate::detection::Detection {
        detector: "reject-burst".into(),
        subject: "venue:venue-a".into(),
        level,
        detail: "three rejections within a minute".into(),
        measured: Some("3".into()),
        threshold: Some("3".into()),
        source: "control".into(),
        evidence: vec![4, 5, 6],
        at_ns: 40,
    }
}

/// A conclusion stands until its detector clears it. The history keeps both, and the strip
/// carries what stands.
#[test]
fn a_detection_stands_until_cleared_and_the_history_keeps_both() {
    use crate::detection::Raised;
    let mut standing = StandingDetectionsPanel::default();
    let mut written = DetectionsPanel::default();
    for level in [Raised::Alarm, Raised::Cleared] {
        let found = detection(level);
        standing.apply(&Entry::Detection(&found));
        written.apply(&Entry::Detection(&found));
        if level == Raised::Alarm {
            assert_eq!(standing.rows().expect("a table").rows.len(), 1);
            assert_eq!(standing.signals()[0].level, crate::panel::Level::Alarm);
        }
    }
    assert!(standing.rows().expect("a table").rows.is_empty());
    assert!(standing.signals().is_empty());
    let rows = written.rows().expect("a table").rows;
    assert_eq!((rows.len(), &rows[1][3]), (2, &Cell::text("cleared")));
    assert_eq!(rows[0][8], Cell::text("4 5 6"));
}
