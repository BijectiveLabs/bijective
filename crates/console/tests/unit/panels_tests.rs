// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Each panel folds only what it is for, and from the whole history, not the latest row.

use bijective_control_engine::wire::{ControlEvent, Participation};
use bijective_data_engine::link::LinkTransition;
use bijective_limits::SafeState;
use bijective_model::{OrderRequest, Side, VenueSymbol};
use bijective_venue::contract::{ConnectionState, DegradedReason, Moment};
use bijective_venue::normalise::{Rejection, Unknown, VenueOutcome};

use super::*;
use crate::panel::{Entry, Level, Panel};

fn control(panel: &mut dyn Panel, events: &[ControlEvent]) {
    for e in events {
        panel.apply(&Entry::Control(e));
    }
}

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

fn settled(outcome: VenueOutcome) -> ControlEvent {
    ControlEvent::Settled { client_id: "bl-1".into(), outcome, at_ns: 20 }
}

fn rendered(panel: &dyn Panel) -> String {
    let mut out = String::new();
    panel.render(&mut out);
    out
}

#[test]
fn an_intent_with_no_settlement_is_shown_as_unanswered() {
    let mut orders = OrdersPanel::default();
    control(&mut orders, &[intent()]);
    assert_eq!(orders.orders.len(), 1);
    assert!(orders.orders[0].settled.is_none());
    assert!(rendered(&orders).contains("unanswered"));
}

#[test]
fn an_unresolved_answer_is_marked_and_raised_and_a_rejection_is_not() {
    let mut open = OrdersPanel::default();
    control(&mut open, &[intent(), settled(VenueOutcome::Unknown(Unknown::Timeout))]);
    assert!(open.orders[0].settled.as_ref().expect("settled").fate_open);
    assert!(rendered(&open).contains("fate_open"));
    assert_eq!(open.signals()[0].level, Level::Alarm);

    let mut refused = OrdersPanel::default();
    let answer = VenueOutcome::Rejected(Rejection::CredentialRefused);
    control(&mut refused, &[intent(), settled(answer)]);
    assert!(!refused.orders[0].settled.as_ref().expect("settled").fate_open);
    assert!(refused.signals().is_empty());
}

/// An answer for an order that is no longer in the kept tail completes nothing, rather than the
/// row that now sits where it was.
#[test]
fn an_answer_for_an_order_out_of_the_tail_completes_no_other_row() {
    let mut orders = OrdersPanel::default();
    for n in 0..=TAIL {
        let mut placed = intent();
        if let ControlEvent::Intent { request, .. } = &mut placed {
            request.client_id = format!("bl-{n}");
        }
        control(&mut orders, &[placed]);
    }
    let late = ControlEvent::Settled {
        client_id: "bl-0".into(),
        outcome: VenueOutcome::Accepted,
        at_ns: 99,
    };
    control(&mut orders, &[late]);
    assert_eq!(orders.orders_seen, TAIL + 1);
    assert!(orders.orders.iter().all(|o| o.settled.is_none()), "another row took the answer");
}

#[test]
fn the_venue_panel_carries_the_cause_and_the_moment() {
    let mut venues = VenuesPanel::default();
    let withdrawn = Participation::Withdrawn { reason: "its account could not be read".into() };
    control(
        &mut venues,
        &[
            ControlEvent::VenueParticipation {
                venue: "venue-a".into(),
                participation: withdrawn,
                at_ns: 10,
            },
            ControlEvent::VenueParticipation {
                venue: "venue-a".into(),
                participation: Participation::Participating,
                at_ns: 20,
            },
        ],
    );
    let row = &venues.venues["venue-a"];
    assert_eq!((row.since_ns, row.changes), (20, 2), "the history was collapsed to the present");
    assert_eq!(row.participation, Participation::Participating);
    assert!(venues.signals().is_empty(), "a venue that came back is not withdrawn");
}

#[test]
fn reconnects_are_counted_from_the_history_not_the_present() {
    let mut links = LinksPanel::default();
    let flapping = [
        ConnectionState::Connecting,
        ConnectionState::Connected { since: Moment::NotTracked },
        ConnectionState::Degraded {
            since: Moment::NotTracked,
            reason: DegradedReason::Stale { last_message: Moment::NotTracked },
        },
        ConnectionState::Connected { since: Moment::NotTracked },
        ConnectionState::Disconnected,
        ConnectionState::Connected { since: Moment::NotTracked },
    ];
    for (n, state) in flapping.into_iter().enumerate() {
        let transition = LinkTransition { state, at_ns: n as i128 };
        links.apply(&Entry::Link { venue: "venue-a", transition: &transition });
    }
    let link = &links.links["venue-a"];
    assert_eq!((link.transitions, link.reconnects, link.degraded), (6, 3, 1));
    assert!(matches!(link.state, ConnectionState::Connected { .. }));
    assert_eq!(links.signals()[0].value, "1 of 1 connected");
}

/// The trading state on the strip is the last one recorded, by a safe state the node drove or by
/// a command an operator carried out, whichever came last.
#[test]
fn the_trading_state_is_the_last_one_recorded() {
    let mut safe = SafeStatePanel::default();
    assert_eq!(safe.signals()[0].level, Level::Attention, "a fresh node is halted");
    control(
        &mut safe,
        &[ControlEvent::CommandApplied {
            command_id: "c-1".into(),
            actor: "ada".into(),
            role: bijective_authority::Role::Operator,
            action: bijective_authority::Action::Resume,
            state: SafeState::Active,
            detail: String::new(),
            from_position: 0,
            at_ns: 5,
        }],
    );
    assert_eq!(safe.trading, Some(SafeState::Active));
    assert_eq!(safe.signals()[0].level, Level::Normal);
}

#[test]
fn every_group_has_a_route_of_its_own_and_at_least_one_panel() {
    let all = groups();
    let mut routes: Vec<&str> = all.iter().map(|g| g.route).collect();
    assert!(all.iter().all(|g| !g.panels.is_empty()));
    routes.sort_unstable();
    routes.dedup();
    assert_eq!(routes.len(), all.len(), "two groups share a route");
}

fn valued(gross: Option<i128>, loss: Option<i128>) -> ControlEvent {
    ControlEvent::PortfolioValued {
        figures: bijective_control_engine::wire::Valued {
            gross_scaled: gross,
            net_scaled: gross,
            daily_loss_scaled: loss,
            observed_at_ns: 1,
            leverage_scaled: None,
            margin_use_scaled: None,
            equity_scaled: None,
        },
        at_ns: 2,
    }
}

fn declared(
    kind: bijective_limits::LimitKind,
    maximum: i128,
) -> bijective_risk::limit_state::wire::LimitEvent {
    bijective_risk::limit_state::wire::LimitEvent::Declared {
        scope: bijective_limits::Scope::Portfolio,
        kind,
        maximum: bijective_limits::Threshold::Money(bijective_limits::Amount::from_scaled(maximum)),
        config_version: 1,
    }
}

/// The strip carries the daily loss against its limit. Past the limit is an alarm, and a limit
/// whose figure is missing is an alarm too, because it cannot be judged.
#[test]
fn the_daily_loss_is_on_the_strip_against_its_limit() {
    let mut exposure = ExposurePanel::default();
    let limit = declared(bijective_limits::LimitKind::DailyLoss, 300_000_000);
    exposure.apply(&Entry::Limit(&limit));
    control(&mut exposure, &[valued(Some(10_000_000_000), Some(100_000_000))]);
    let loss = exposure.signals().into_iter().find(|s| s.label == "daily loss").expect("shown");
    assert_eq!((loss.value.as_str(), loss.level), ("1.00 of 3.00", Level::Normal));

    control(&mut exposure, &[valued(Some(10_000_000_000), Some(300_000_000))]);
    let loss = exposure.signals().into_iter().find(|s| s.label == "daily loss").expect("shown");
    assert_eq!(loss.level, Level::Alarm, "at the limit is a breach");

    control(&mut exposure, &[valued(None, None)]);
    let loss = exposure.signals().into_iter().find(|s| s.label == "daily loss").expect("shown");
    assert_eq!((loss.value.as_str(), loss.level), ("unmeasured", Level::Alarm));
    assert!(rendered(&exposure).contains("unmeasured"));
}

/// Leverage and margin use show beside the money figures once a limit declares them, as
/// multiples and percentages. Unmeasured against a declared limit is an alarm.
#[test]
fn leverage_and_margin_use_show_against_their_limits() {
    use bijective_limits::{LimitKind, Ratio, Threshold};
    let mut exposure = ExposurePanel::default();
    control(&mut exposure, &[valued(Some(10_000_000_000), None)]);
    assert!(!rendered(&exposure).contains("leverage"), "nothing declared it");

    let limit = bijective_risk::limit_state::wire::LimitEvent::Declared {
        scope: bijective_limits::Scope::Portfolio,
        kind: LimitKind::Leverage,
        maximum: Threshold::Ratio(Ratio::from_scaled(30_000)),
        config_version: 1,
    };
    exposure.apply(&Entry::Limit(&limit));
    let leverage = |p: &ExposurePanel| p.signals().into_iter().find(|s| s.label == "leverage");
    let unmeasured = leverage(&exposure).expect("shown");
    assert_eq!((unmeasured.value.as_str(), unmeasured.level), ("unmeasured", Level::Alarm));

    let ControlEvent::PortfolioValued { mut figures, at_ns } = valued(Some(10_000_000_000), None)
    else {
        unreachable!()
    };
    figures.leverage_scaled = Some(25_000);
    figures.margin_use_scaled = Some(3_500);
    control(&mut exposure, &[ControlEvent::PortfolioValued { figures, at_ns }]);
    let measured = leverage(&exposure).expect("shown");
    assert_eq!((measured.value.as_str(), measured.level), ("2.50x of 3.00x", Level::Attention));
    let out = rendered(&exposure);
    assert!(out.contains("35.00%") && out.contains("margin use"), "{out}");
}

/// A held leg shows as held until released, and its parent and group are named.
#[test]
fn a_held_proposal_shows_until_it_is_released() {
    let proposal = bijective_model::Proposal {
        desk: "alpha".into(),
        instrument: bijective_model::InstrumentId::new("aaa"),
        side: Side::Sell,
        quantity_scaled: 1_000,
        price_scaled: Some(300),
        cap_scaled: None,
        client_id: "d-1".into(),
        decided_at_position: None,
        decided_at_ns: 1,
        venue: None,
        strategy: None,
        terms: Default::default(),
        reduce_only: true,
        parent: Some("entry".into()),
        oco: Some("bracket".into()),
    };
    let mut held = HeldPanel::default();
    assert!(rendered(&held).contains("No proposal has been held"));
    control(&mut held, &[ControlEvent::Held { proposal, from_position: 1, at_ns: 5 }]);
    let out = rendered(&held);
    assert!(out.contains("entry") && out.contains("bracket") && out.contains("held"), "{out}");
    assert_eq!(held.signals()[0].label, "held proposals");

    control(&mut held, &[intent()]);
    assert!(rendered(&held).contains("released"));
    assert!(held.signals().is_empty(), "nothing left held, and every entry folded");
}

#[test]
fn a_book_top_shows_its_spread_and_one_side_empty_is_named() {
    use bijective_serialization::signal::market::BookUpdate;
    let mut market = MarketPanel::default();
    let mut update = BookUpdate {
        instrument: bijective_model::InstrumentId::new("btc-perp"),
        seq: bijective_model::Sequence::NotNumbered,
        venue_ts_ms: 1,
        received_at_ns: None,
        bid_price: 9_990,
        bid_quantity: 1,
        ask_price: 10_010,
        ask_quantity: 1,
        bid_depth: 5,
        ask_depth: 4,
    };
    market.apply(&Entry::Book { venue: "venue-a", update: &update });
    assert!(rendered(&market).contains("<td>20<td>"), "{}", rendered(&market));
    update.bid_price = 0;
    market.apply(&Entry::Book { venue: "venue-a", update: &update });
    assert!(rendered(&market).contains("one side empty"));
    assert_eq!(market.tops.len(), 1, "one instrument on one venue is one row");
}

#[test]
fn a_scaled_figure_shows_every_recorded_digit_and_none_added() {
    use crate::markup::decimal;
    assert_eq!(decimal(6_612_345, 2), "66123.45");
    assert_eq!(decimal(-5_671, 8), "-0.00005671");
    assert_eq!(decimal(7, 0), "7");
    assert_eq!(decimal(0, 3), "0.000");
}

#[test]
fn the_liquidations_kept_are_the_newest_and_trades_do_not_enter_them() {
    use bijective_archive::{FactKind, FactRow, FactSide};
    let row = |n: u64, kind| FactRow {
        journal_seq: n,
        record_in_frame: 0,
        received_at_ns: n as i64,
        exchange_time_ns: None,
        instrument: "BTCUSDT".into(),
        kind,
        side: Some(FactSide::Buy),
        trade_id: None,
        price_scaled: Some(100),
        quantity_scaled: Some(1),
        value_scaled: None,
        value_scale: None,
        next_funding_ns: None,
    };
    let mut panel = LiquidationsPanel::default();
    for n in 0..60 {
        let r = row(n, FactKind::Liquidation);
        panel.apply(&Entry::Fact { venue: "venue-a", price_scale: 2, quantity_scale: 0, row: &r });
    }
    let trade = row(99, FactKind::Trade);
    panel.apply(&Entry::Fact { venue: "venue-a", price_scale: 2, quantity_scale: 0, row: &trade });
    assert_eq!(panel.forced.len(), 50);
    assert_eq!(panel.forced.first().map(|f| f.row.journal_seq), Some(10));
    assert_eq!(panel.forced.last().map(|f| f.row.journal_seq), Some(59));
}

fn intent_on(client_id: &str, venue: Option<&str>, at_ns: i128) -> ControlEvent {
    let mut placed = intent();
    if let ControlEvent::Intent { venue: named, request, at_ns: when, .. } = &mut placed {
        *named = venue.map(str::to_string);
        request.client_id = client_id.into();
        *when = at_ns;
    }
    placed
}

fn answer(client_id: &str, at_ns: i128) -> ControlEvent {
    ControlEvent::Settled { client_id: client_id.into(), outcome: VenueOutcome::Accepted, at_ns }
}

/// The span is intent to settlement per venue. An intent with no venue is counted, not guessed,
/// and one still unanswered is shown as waiting.
#[test]
fn latency_is_measured_per_venue_from_intent_to_answer() {
    let mut panel = LatencyPanel::default();
    control(
        &mut panel,
        &[
            intent_on("a-1", Some("venue-a"), 0),
            answer("a-1", 4_000_000),
            intent_on("a-2", Some("venue-a"), 10_000_000),
            answer("a-2", 22_500_000),
            intent_on("b-1", Some("venue-b"), 0),
            answer("b-1", 30_000_000),
            intent_on("old", None, 0),
            answer("old", 1),
            intent_on("waiting", Some("venue-b"), 5),
        ],
    );
    let a = &panel.venues["venue-a"];
    assert_eq!(
        (a.answered, a.percentile(50), a.percentile(100)),
        (2, Some(4_000_000), Some(12_500_000))
    );
    assert_eq!(panel.venues["venue-b"].percentile(95), Some(30_000_000));
    assert_eq!((panel.unattributed, panel.unanswered()), (1, 1));
    let html = rendered(&panel);
    assert!(html.contains("<td>venue-a<td>2<td>4.0<td>12.5<td>12.5"), "{html}");
}

fn declared_at(
    scope: bijective_limits::Scope,
    maximum: i128,
) -> bijective_risk::limit_state::wire::LimitEvent {
    bijective_risk::limit_state::wire::LimitEvent::Declared {
        scope,
        kind: bijective_limits::LimitKind::GrossExposure,
        maximum: bijective_limits::Threshold::Money(bijective_limits::Amount::from_scaled(maximum)),
        config_version: 1,
    }
}

/// Each scope's figure stands beside its limit. A declared limit whose figure the last record
/// does not hold is unmeasured, and that is the alarm on the strip.
#[test]
fn a_scope_limit_without_its_figure_is_an_alarm_and_one_near_it_asks_for_attention() {
    use bijective_control_engine::wire::ScopeFigure;
    use bijective_limits::{LimitKind, Scope};

    let mut panel = crate::panels::ScopesPanel::default();
    panel.apply(&Entry::Limit(&declared_at(Scope::Desk("alpha".into()), 1_000)));
    panel.apply(&Entry::Limit(&declared_at(Scope::Underlying("btc".into()), 1_000)));
    let figure =
        |scope, value| ScopeFigure { scope, kind: LimitKind::GrossExposure, value_scaled: value };
    let record = ControlEvent::ScopesValued {
        figures: vec![figure(Scope::Desk("alpha".into()), 900)],
        at_ns: 5,
    };
    panel.apply(&Entry::Control(&record));

    let mut out = String::new();
    panel.render(&mut out);
    assert!(out.contains("desk:alpha") && out.contains("underlying:btc"), "{out}");
    let signals = panel.signals();
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].level, Level::Alarm, "{signals:?}");
    assert!(signals[0].value.contains("underlying:btc gross unmeasured"), "{signals:?}");

    let both = ControlEvent::ScopesValued {
        figures: vec![
            figure(Scope::Desk("alpha".into()), 900),
            figure(Scope::Underlying("btc".into()), 100),
        ],
        at_ns: 6,
    };
    panel.apply(&Entry::Control(&both));
    let signals = panel.signals();
    assert_eq!(signals[0].level, Level::Attention, "nine tenths of the desk's limit: {signals:?}");
}
