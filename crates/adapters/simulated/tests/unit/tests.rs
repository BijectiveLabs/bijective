// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The matching rules, each shown by the market that triggers it, and the parity that the venue's
//! evidence reconciles with a platform ledger built from the fills it reported.

use bijective_lifecycle::{Ledger, Liquidity, OrderEvent, OrderState, Verdict, reconcile};
use bijective_model::TimeInForce;
use bijective_model::market::{
    BookDelta, BookLevel, BookSide, BookSnapshot, ExchangeTime, LevelChange, MarketFact,
    MarketRecord,
};
use bijective_model::{Clearance, OrderRequest, Sequence, Side, VenueSymbol};
use bijective_venue::contract::VenueExecution;
use bijective_venue::contract::conformance::{CheckOutcome, check_execution};
use bijective_venue::normalise::{Rejection, VenueOutcome};
use bijective_venue::profile::{CapabilityDeclaration, VenueProfile};

use crate::{Engine, FillModel, LatencyModel, SimulatedVenue, SimulationModel};

fn declaration() -> CapabilityDeclaration {
    let profile = r#"{
        "format": 1, "venue": "simulated", "verified_at": null,
        "transports": { "stream": { "funding": { "model": "continuous" }, "data_classes": ["depth"], "max_staleness_ns": "1000000000" } },
        "reduce_only": "placement-precondition", "order_types": [{ "name": "limit", "native": true }],
        "instruments": [{ "symbol": "AAA", "id": "sim.aaa", "kind": "perpetual", "settlement_currency": "usdt",
            "quote_currency": "usdt",
            "precision": { "price_scale": 2, "quantity_scale": 3, "price_tick": "10", "quantity_lot": "1",
                "price_validity": { "rule": "increment-only" } },
            "minimum_size": "2", "minimum_notional": "1", "status": "trading" }],
        "identifier_rules": { "allowed": "abcdefghijklmnopqrstuvwxyz0123456789-", "max_len": 36 },
        "transformations": [],
        "support": { "venue": "simulated", "depth": "full", "debuggable_to_root_cause": true }
    }"#;
    CapabilityDeclaration::from_profile(&VenueProfile::from_json(profile).expect("profile"))
}

fn model(fill: FillModel, latency: i128) -> SimulationModel {
    SimulationModel { fill, latency: LatencyModel { submit_ns: latency, cancel_ns: latency } }
}

fn level(price_scaled: i128, quantity_scaled: i128) -> BookLevel {
    BookLevel { price_scaled, quantity_scaled }
}

fn record(at: i128, fact: MarketFact) -> MarketRecord {
    MarketRecord {
        instrument: VenueSymbol::new("AAA"),
        exchange_time: ExchangeTime::At { at_ns: at },
        received_at_ns: at,
        fact,
    }
}

/// Bids 990 and 980, asks 1000 (5) and 1010 (5).
fn book(at: i128) -> MarketRecord {
    record(
        at,
        MarketFact::BookSnapshot(BookSnapshot {
            sequence: Sequence::Numbered(1),
            bids: vec![level(990, 5), level(980, 5)],
            asks: vec![level(1_000, 5), level(1_010, 5)],
        }),
    )
}

fn ask_moves_to(at: i128, price: i128) -> MarketRecord {
    record(
        at,
        MarketFact::BookDelta(BookDelta {
            sequence: Sequence::Numbered(2),
            changes: vec![
                LevelChange::Remove { side: BookSide::Ask, price_scaled: 1_000 },
                LevelChange::Remove { side: BookSide::Ask, price_scaled: 1_010 },
                LevelChange::Set { side: BookSide::Ask, price_scaled: price, quantity_scaled: 9 },
            ],
        }),
    )
}

fn order(id: &str, side: Side, quantity: i128, price: Option<i128>) -> OrderRequest {
    OrderRequest {
        client_id: id.into(),
        instrument: "AAA".into(),
        side,
        quantity_scaled: quantity,
        price_scaled: price,
        reduce_only: false,
        terms: Default::default(),
    }
}

fn state(engine: &Engine, id: &str) -> (OrderState, i128) {
    let o = engine.ledger().order(id).expect("held");
    (o.state, o.filled_quantity_scaled)
}

#[test]
fn an_arriving_order_takes_the_book_best_first_at_each_levels_price() {
    let mut e = Engine::new(model(FillModel::TradeThrough, 0));
    e.on_record(&book(1));
    assert_eq!(
        e.submit(&declaration(), &order("a", Side::Buy, 8, Some(1_010)), 2),
        VenueOutcome::Accepted
    );
    e.settle(2);
    let prices: Vec<(i128, i128, Liquidity)> = e
        .fills_since(0)
        .iter()
        .map(|f| (f.fill.price_scaled, f.fill.quantity_scaled, f.fill.liquidity))
        .collect();
    assert_eq!(prices, vec![(1_000, 5, Liquidity::Taker), (1_010, 3, Liquidity::Taker)]);
    assert_eq!(state(&e, "a"), (OrderState::Filled, 8));
}

/// Liquidity a fill took is gone until the venue's record restates the level.
#[test]
fn one_displayed_quantity_is_never_filled_twice() {
    let mut e = Engine::new(model(FillModel::TradeThrough, 0));
    e.on_record(&book(1));
    e.submit(&declaration(), &order("a", Side::Buy, 5, Some(1_000)), 2);
    e.submit(&declaration(), &order("b", Side::Buy, 5, Some(1_000)), 2);
    e.settle(2);
    assert_eq!(state(&e, "a"), (OrderState::Filled, 5));
    assert_eq!(state(&e, "b"), (OrderState::Accepted, 0), "the level was already taken");
}

#[test]
fn a_command_takes_effect_only_after_its_latency() {
    let mut e = Engine::new(model(FillModel::TradeThrough, 5));
    e.on_record(&book(1));
    e.submit(&declaration(), &order("a", Side::Buy, 2, Some(1_000)), 10);
    e.settle(14);
    assert_eq!(state(&e, "a"), (OrderState::Accepted, 0));
    e.settle(15);
    assert_eq!(state(&e, "a"), (OrderState::Filled, 2));
}

/// A resting order's place in the queue is unknown, so a touch does not fill it. A trade through does.
#[test]
fn a_resting_order_fills_on_a_trade_through_at_its_own_price() {
    let mut e = Engine::new(model(FillModel::TradeThrough, 0));
    e.on_record(&book(1));
    e.submit(&declaration(), &order("a", Side::Buy, 4, Some(990)), 2);
    e.settle(2);
    e.on_record(&ask_moves_to(3, 990));
    assert_eq!(state(&e, "a"), (OrderState::Accepted, 0), "a touch is not a fill");
    e.on_record(&ask_moves_to(4, 980));
    assert_eq!(state(&e, "a"), (OrderState::Filled, 4));
    let fill = &e.fills_since(0)[0].fill;
    assert_eq!((fill.price_scaled, fill.liquidity), (990, Liquidity::Maker));
}

#[test]
fn the_touch_model_fills_at_the_limit() {
    let mut e = Engine::new(model(FillModel::Touch, 0));
    e.on_record(&book(1));
    e.submit(&declaration(), &order("a", Side::Buy, 4, Some(990)), 2);
    e.settle(2);
    e.on_record(&ask_moves_to(3, 990));
    assert_eq!(state(&e, "a"), (OrderState::Filled, 4));
}

#[test]
fn a_market_order_does_not_rest_what_it_could_not_fill() {
    let mut e = Engine::new(model(FillModel::TradeThrough, 0));
    e.on_record(&book(1));
    e.submit(&declaration(), &order("m", Side::Sell, 12, None), 2);
    e.settle(2);
    assert_eq!(state(&e, "m"), (OrderState::Canceled, 10));
}

/// A fill that happens while a cancel is in flight counts. The cancel then closes the rest.
#[test]
fn a_fill_that_crosses_a_cancel_counts() {
    let mut e = Engine::new(model(FillModel::TradeThrough, 5));
    e.on_record(&book(1));
    e.submit(&declaration(), &order("a", Side::Buy, 9, Some(990)), 1);
    e.settle(6);
    assert_eq!(e.cancel("a", 7), VenueOutcome::Accepted);
    e.on_record(&ask_moves_to(8, 980));
    e.settle(12);
    assert_eq!(
        state(&e, "a"),
        (OrderState::Filled, 9),
        "the fill completed it before the cancel arrived"
    );
}

#[test]
fn the_venue_refuses_what_a_venue_would_refuse() {
    let d = declaration();
    let mut e = Engine::new(model(FillModel::TradeThrough, 0));
    let mut unknown = order("u", Side::Buy, 2, Some(1_000));
    unknown.instrument = "ZZZ".into();
    assert_eq!(e.submit(&d, &unknown, 1), VenueOutcome::Rejected(Rejection::UnknownInstrument));
    assert_eq!(
        e.submit(&d, &order("s", Side::Buy, 1, Some(1_000)), 1),
        VenueOutcome::Rejected(Rejection::BelowMinimumSize)
    );
    assert!(matches!(
        e.submit(&d, &order("t", Side::Buy, 2, Some(1_005)), 1),
        VenueOutcome::Rejected(Rejection::Unclassified { .. })
    ));
    assert_eq!(e.submit(&d, &order("a", Side::Buy, 2, Some(1_000)), 1), VenueOutcome::Accepted);
    assert_eq!(
        e.submit(&d, &order("a", Side::Buy, 2, Some(1_000)), 1),
        VenueOutcome::Rejected(Rejection::Duplicate)
    );
    assert!(matches!(e.cancel("nothing", 1), VenueOutcome::Rejected(_)));
}

/// The parity the simulated venue exists for: the platform's ledger, built from the fills the venue
/// reported, reconciles with the venue's own evidence.
#[test]
fn the_platform_ledger_built_from_reported_fills_reconciles_with_the_venue() {
    let mut e = Engine::new(model(FillModel::TradeThrough, 0));
    e.on_record(&book(1));
    let placed = [
        order("a", Side::Buy, 8, Some(1_010)),
        order("b", Side::Sell, 3, Some(990)),
        order("c", Side::Buy, 2, Some(970)),
    ];
    let mut ours = Ledger::default();
    for o in &placed {
        assert_eq!(e.submit(&declaration(), o, 2), VenueOutcome::Accepted);
        ours.create(o.clone()).unwrap();
        ours.apply(&o.client_id, &OrderEvent::Submitted, 2).unwrap();
        ours.apply(&o.client_id, &OrderEvent::Accepted { venue_order_id: None }, 2).unwrap();
    }
    e.settle(2);
    for report in e.fills_since(0) {
        ours.apply(
            report.client_id.as_deref().unwrap(),
            &OrderEvent::Filled(report.fill.clone()),
            3,
        )
        .unwrap();
    }
    let r = reconcile(&ours, &e.evidence(), 4);
    assert_eq!(r.verdict, Verdict::Aligned, "{:?}", r.findings);
    assert_eq!(r.ledger.position(&VenueSymbol::new("AAA")).unwrap().quantity_scaled(), 5);
    assert!(e.faults().is_empty());
}

/// A missed fill is found: reconciliation is exercised by the simulation, not only in production.
#[test]
fn a_fill_the_platform_missed_is_found_by_reconciliation() {
    let mut e = Engine::new(model(FillModel::TradeThrough, 0));
    e.on_record(&book(1));
    let o = order("a", Side::Buy, 5, Some(1_000));
    e.submit(&declaration(), &o, 2);
    e.settle(2);
    let mut ours = Ledger::default();
    ours.create(o).unwrap();
    ours.apply("a", &OrderEvent::Submitted, 2).unwrap();
    let mut evidence = e.evidence();
    evidence.fills.clear();
    let r = reconcile(&ours, &evidence, 4);
    assert_eq!(r.verdict, Verdict::Diverged);
}

#[test]
fn the_same_inputs_give_the_same_venue_record() {
    let run = || {
        let mut e = Engine::new(model(FillModel::TradeThrough, 3));
        e.on_record(&book(1));
        e.submit(&declaration(), &order("a", Side::Buy, 4, Some(990)), 2);
        e.submit(&declaration(), &order("b", Side::Sell, 6, None), 2);
        e.settle(5);
        e.on_record(&ask_moves_to(6, 980));
        e.settle(6);
        e.evidence()
    };
    assert_eq!(run(), run());
}

#[tokio::test]
async fn behind_the_contract_it_answers_as_a_venue_and_shows_its_positions() {
    let mut venue = SimulatedVenue::new(declaration(), model(FillModel::TradeThrough, 0));
    venue.engine_mut().on_record(&book(1));
    let clearance = Clearance::granted(0, 0, true, 1).expect("cleared");
    let outcome = venue.submit(&clearance, &order("a", Side::Buy, 8, Some(1_010)), 2).await;
    assert_eq!(outcome, VenueOutcome::Accepted);
    venue.engine_mut().settle(2);
    let account = venue.account(3).await.expect("account");
    assert_eq!(account.positions.len(), 1);
    // (5 * 1000 + 3 * 1010) / 8 = 1003.75, shown as 1003 as a venue would display it.
    assert_eq!(
        (account.positions[0].quantity_scaled, account.positions[0].entry_price_scaled),
        (8, 1_003)
    );
    assert!(venue.engine().model().describe().contains("TradeThrough"));
}

/// The same checks every adapter's execution side is held to: a resubmitted identifier is not a
/// second order, an unknown is never a rejection, and the account is readable for reconciliation.
#[tokio::test]
async fn the_simulated_venue_passes_the_execution_conformance_checks() {
    let mut venue = SimulatedVenue::new(declaration(), model(FillModel::TradeThrough, 0));
    venue.engine_mut().on_record(&book(1));
    let clearance = Clearance::granted(0, 0, true, 1).expect("cleared");

    let results =
        check_execution(&mut venue, &clearance, &order("a", Side::Buy, 8, Some(1_010)), 2).await;

    let passed = |name: &str| {
        results.iter().any(|r| r.name == name && matches!(r.outcome, CheckOutcome::Passed))
    };
    for name in [
        "resubmission-is-idempotent",
        "unknown-is-not-a-rejection",
        "account-readable-for-reconciliation",
    ] {
        assert!(passed(name), "{name} did not pass: {results:?}");
    }
    // The one check left needs a live venue and is not releasable until one has run it.
    assert!(!results.iter().any(|r| matches!(r.outcome, CheckOutcome::Failed { .. })));
}

fn sandbox(cash: i128) -> Engine {
    let account = crate::account::Account::new(
        [("usdt".to_string(), cash)].into_iter().collect(),
        crate::account::FeeRates { taker_bps_scaled: Some(1_000_000_000), maker_bps_scaled: None },
    );
    Engine::with_account(model(FillModel::TradeThrough, 0), account)
}

/// A buy the account cannot pay for, fee included, is refused. One it can pay for reserves its
/// cost, so a second buy cannot spend the same balance (decision 0003). Money at eight decimals:
/// five lots at 10.00 cost 0.05 and a 10 bps taker fee 0.00005.
#[test]
fn a_buy_the_sandbox_account_cannot_pay_for_is_refused() {
    let mut e = sandbox(6_000_000);
    e.on_record(&book(1));
    assert_eq!(
        e.submit(&declaration(), &order("a", Side::Buy, 5, Some(1_000)), 2),
        VenueOutcome::Accepted
    );
    assert_eq!(
        e.submit(&declaration(), &order("b", Side::Buy, 2, Some(1_000)), 2),
        VenueOutcome::Rejected(Rejection::InsufficientMargin)
    );
}

/// A fill moves cash and carries the fee charged. A sell is paid its proceeds less the fee.
#[test]
fn every_sandbox_fill_moves_cash_and_carries_its_fee() {
    let mut e = sandbox(6_000_000);
    e.on_record(&book(1));
    e.submit(&declaration(), &order("a", Side::Buy, 5, Some(1_000)), 2);
    e.settle(2);
    let fee = e.fills_since(0)[0].fill.fee.clone().expect("a declared taker fee");
    assert_eq!((fee.asset.as_str(), fee.amount_scaled, fee.scale), ("usdt", 5_000, 8));
    assert_eq!(e.account().expect("account").cash("usdt"), 995_000);

    e.submit(&declaration(), &order("s", Side::Sell, 5, Some(990)), 3);
    e.settle(3);
    assert_eq!(e.account().expect("account").cash("usdt"), 995_000 + 4_950_000 - 4_950);
}

/// A canceled buy gives back what it reserved.
#[test]
fn a_canceled_buy_releases_its_reservation() {
    let mut e = sandbox(6_000_000);
    e.on_record(&book(1));
    e.submit(&declaration(), &order("a", Side::Buy, 5, Some(900)), 2);
    e.settle(2);
    assert_eq!(
        e.submit(&declaration(), &order("b", Side::Buy, 5, Some(900)), 2),
        VenueOutcome::Rejected(Rejection::InsufficientMargin)
    );
    e.cancel("a", 3);
    e.settle(3);
    assert_eq!(
        e.submit(&declaration(), &order("b", Side::Buy, 5, Some(900)), 4),
        VenueOutcome::Accepted
    );
}

fn with_terms(
    mut request: OrderRequest,
    time_in_force: TimeInForce,
    post_only: bool,
) -> OrderRequest {
    request.terms = bijective_model::OrderTerms {
        time_in_force,
        post_only,
        stop_price_scaled: None,
        expire_at_ns: None,
    };
    request
}

/// Immediate-or-cancel takes what crosses and cancels the rest. Fill-or-kill fills whole or not at
/// all. Post-only that would take is cancelled untouched, and rests when it would not (decision 0009).
#[test]
fn immediate_fill_or_kill_and_post_only_orders_meet_the_book_as_their_terms_say() {
    let mut e = Engine::new(model(FillModel::TradeThrough, 0));
    e.on_record(&book(1));
    let buy = |id, quantity, price, time_in_force, post_only| {
        with_terms(order(id, Side::Buy, quantity, Some(price)), time_in_force, post_only)
    };
    e.submit(&declaration(), &buy("a", 8, 1_000, TimeInForce::ImmediateOrCancel, false), 2);
    e.settle(2);
    assert_eq!(state(&e, "a"), (OrderState::Canceled, 5), "five crossed, the rest cancelled");

    e.on_record(&book(3));
    e.submit(&declaration(), &buy("b", 8, 1_000, TimeInForce::FillOrKill, false), 4);
    e.settle(4);
    assert_eq!(state(&e, "b"), (OrderState::Canceled, 0), "five of eight is not whole");

    e.submit(&declaration(), &buy("c", 5, 1_000, TimeInForce::GoodTillCancelled, true), 5);
    e.submit(&declaration(), &buy("ab", 5, 990, TimeInForce::GoodTillCancelled, true), 5);
    e.settle(5);
    assert_eq!(state(&e, "c"), (OrderState::Canceled, 0));
    assert_eq!(state(&e, "ab"), (OrderState::Accepted, 0));
}

/// The day, the open and the close need a session calendar the simulated venue does not keep.
#[test]
fn session_terms_are_refused_unsent() {
    let mut e = Engine::new(model(FillModel::TradeThrough, 0));
    e.on_record(&book(1));
    for term in [TimeInForce::Day, TimeInForce::AtTheOpen, TimeInForce::AtTheClose] {
        let refused = e.submit(
            &declaration(),
            &with_terms(order("a", Side::Buy, 5, Some(990)), term, false),
            2,
        );
        assert!(matches!(refused, VenueOutcome::Rejected(_)), "{term:?}: {refused:?}");
    }
}

/// A stop waits until the book reaches it, then executes as the order it carries (decision 0012).
#[test]
fn a_stop_waits_for_the_book_then_executes() {
    let mut e = Engine::new(model(FillModel::TradeThrough, 0));
    e.on_record(&book(1));
    let mut buy = order("s", Side::Buy, 3, None);
    buy.terms.stop_price_scaled = Some(1_005);
    assert_eq!(e.submit(&declaration(), &buy, 2), VenueOutcome::Accepted);
    e.settle(2);
    assert_eq!(state(&e, "s"), (OrderState::Accepted, 0), "held: the ask is below the stop");
    e.on_record(&ask_moves_to(3, 1_006));
    assert_eq!(state(&e, "s"), (OrderState::Filled, 3), "triggered and taken at the market");
    assert_eq!(e.fills_since(0)[0].fill.price_scaled, 1_006);
}

/// A paper venue rebuilt from its record (decision 0018) holds what it held: the same orders,
/// fills, positions and cash, and a resting order meets the next market as it would have.
#[test]
fn a_venue_restored_from_its_record_continues_as_it_was() {
    let mut first = sandbox(100_000_000_000);
    first.on_record(&book(1));
    first.submit(&declaration(), &order("taken", Side::Buy, 5, Some(1_000)), 2);
    first.submit(&declaration(), &order("waits", Side::Buy, 4, Some(980)), 2);
    first.submit(&declaration(), &order("gone", Side::Buy, 3, Some(970)), 2);
    first.settle(2);
    first.cancel("gone", 3);
    first.settle(3);
    let record = first.ledger().clone();

    let mut again = sandbox(100_000_000_000);
    let restored = again.restore(&declaration(), &record, 10);
    assert_eq!((restored.orders, restored.fills, restored.working), (3, 1, 1));
    assert!(again.faults().is_empty(), "{:?}", again.faults());
    let usdt = |e: &Engine| e.account().map(|a| a.cash("usdt"));
    assert_eq!(usdt(&again), usdt(&first), "the cash the fills left");
    assert_eq!(again.evidence(), first.evidence(), "the venue reports what it reported before");

    for engine in [&mut first, &mut again] {
        engine.on_record(&book(11));
        engine.on_record(&ask_moves_to(12, 970));
    }
    assert_eq!(state(&again, "waits"), (OrderState::Filled, 4));
    assert_eq!(again.evidence(), first.evidence(), "and it goes on the same way");

    let next = order("later", Side::Buy, 2, Some(1_000));
    assert_eq!(again.submit(&declaration(), &next, 13), VenueOutcome::Accepted);
    let ids: Vec<&str> =
        again.ledger().orders().filter_map(|o| o.venue_order_id.as_deref()).collect();
    assert!(ids.contains(&"sim-4"), "a new order follows the restored identifiers: {ids:?}");
}
