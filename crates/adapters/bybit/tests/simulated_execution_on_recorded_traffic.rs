// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The recorded run, through the live feed, drives the simulated venue: orders placed into real
//! book dynamics fill under the declared models, deterministically, and the venue's evidence
//! reconciles with a platform ledger built from the fills it reported.

use bijective_lifecycle::{Ledger, OrderEvent, Verdict, reconcile};
use bijective_model::{OrderRequest, Side};
use bijective_venue::contract::{MarketFeed, MarketRecord};
use bijective_venue::normalise::{Scale, VenueOutcome};
use bijective_venue::profile::{CapabilityDeclaration, VenueProfile};
use bijective_venue_bybit::Feed;
use bijective_venue_simulated::{Engine, FillModel, LatencyModel, SimulationModel};
use serde::Deserialize;

const FIXTURE: &str = include_str!("../fixtures/bybit_linear_orderbook50.jsonl");

#[derive(Deserialize)]
struct Recorded {
    at_ns: String,
    frame: String,
}

fn records() -> Vec<MarketRecord> {
    let mut feed = Feed::new(bijective_venue::profile::InstrumentScales::uniform_over(
        ["BTCUSDT", "ETHUSDT"],
        bijective_venue::profile::Scales {
            price: Scale::new(2).unwrap(),
            quantity: Scale::new(3).unwrap(),
        },
    ));
    let mut out = Vec::new();
    for line in FIXTURE.lines().filter(|l| !l.trim().is_empty()) {
        let rec: Recorded = serde_json::from_str(line).expect("fixture line");
        let offered = feed
            .offer(rec.frame.as_bytes(), rec.at_ns.parse().expect("arrival"))
            .expect("accepted");
        out.extend(offered.records);
    }
    out
}

fn declaration() -> CapabilityDeclaration {
    let instrument = |s: &str| {
        serde_json::json!({ "symbol": s, "id": s, "kind": "perpetual", "settlement_currency": "usdt",
            "quote_currency": "usdt", "precision": { "price_scale": 2, "quantity_scale": 3, "price_tick": "10",
            "quantity_lot": "1", "price_validity": { "rule": "increment-only" } }, "minimum_size": "1",
            "minimum_notional": "1", "status": "trading" })
    };
    let profile = serde_json::json!({
        "format": 1, "venue": "the venue under replay", "verified_at": null,
        "transports": { "stream": { "funding": { "model": "continuous" }, "data_classes": ["depth"], "max_staleness_ns": "1000000000" } },
        "reduce_only": "placement-precondition", "order_types": [{ "name": "limit", "native": true }],
        "instruments": [instrument("BTCUSDT"), instrument("ETHUSDT")],
        "identifier_rules": { "allowed": "abcdefghijklmnopqrstuvwxyz0123456789-", "max_len": 36 },
        "transformations": [],
        "support": { "venue": "the venue under replay", "depth": "full", "debuggable_to_root_cause": true }
    });
    CapabilityDeclaration::from_profile(
        &VenueProfile::from_json(&profile.to_string()).expect("profile"),
    )
}

/// Every 200th record, a market order of 0.005 alternating sides on that record's instrument.
/// Every 300th, a limit buy one tick under the best bid, left to rest.
fn run(model: SimulationModel) -> (Engine, Ledger) {
    let declaration = declaration();
    let mut engine = Engine::new(model);
    let mut ours = Ledger::default();
    let mut placed = 0;
    for (n, record) in records().iter().enumerate() {
        engine.on_record(record);
        let place = |order: OrderRequest, engine: &mut Engine, ours: &mut Ledger| {
            if engine.submit(&declaration, &order, record.received_at_ns) == VenueOutcome::Accepted
            {
                ours.create(order.clone()).unwrap();
                ours.apply(&order.client_id, &OrderEvent::Submitted, record.received_at_ns)
                    .unwrap();
                ours.apply(
                    &order.client_id,
                    &OrderEvent::Accepted { venue_order_id: None },
                    record.received_at_ns,
                )
                .unwrap();
            }
        };
        if n % 200 == 0 {
            placed += 1;
            let side = if placed % 2 == 0 { Side::Sell } else { Side::Buy };
            let order = OrderRequest {
                client_id: format!("m-{n}"),
                instrument: record.instrument.clone(),
                side,
                quantity_scaled: 5,
                price_scaled: None,
                reduce_only: false,
                terms: Default::default(),
            };
            place(order, &mut engine, &mut ours);
        }
        if n % 300 == 0
            && let Some(top) = book_bid(record)
        {
            let order = OrderRequest {
                client_id: format!("l-{n}"),
                instrument: record.instrument.clone(),
                side: Side::Buy,
                quantity_scaled: 5,
                price_scaled: Some(top - 10),
                reduce_only: false,
                terms: Default::default(),
            };
            place(order, &mut engine, &mut ours);
        }
        engine.settle(record.received_at_ns);
    }
    for report in engine.fills_since(0) {
        ours.apply(
            report.client_id.as_deref().unwrap(),
            &OrderEvent::Filled(report.fill.clone()),
            0,
        )
        .unwrap();
    }
    (engine, ours)
}

fn book_bid(record: &MarketRecord) -> Option<i128> {
    match &record.fact {
        bijective_venue::contract::MarketFact::BookSnapshot(s) => {
            s.bids.first().map(|l| l.price_scaled)
        }
        _ => None,
    }
}

fn model(fill: FillModel) -> SimulationModel {
    SimulationModel { fill, latency: LatencyModel { submit_ns: 50_000_000, cancel_ns: 50_000_000 } }
}

#[test]
fn orders_placed_into_the_recorded_run_fill_and_reconcile() {
    let (engine, ours) = run(model(FillModel::TradeThrough));
    assert!(engine.faults().is_empty(), "{:?}", engine.faults());
    assert!(
        engine.fills_since(0).len() >= 10,
        "the run produced {} fills",
        engine.fills_since(0).len()
    );
    let r = reconcile(&ours, &engine.evidence(), 1);
    assert_eq!(r.verdict, Verdict::Aligned, "{:?}", r.findings);
}

#[test]
fn the_recorded_run_simulates_identically_twice() {
    let (first, _) = run(model(FillModel::TradeThrough));
    let (second, _) = run(model(FillModel::TradeThrough));
    assert_eq!(first.evidence(), second.evidence());
}

/// The models are inputs and change the result: on this run the optimistic model fills at least
/// what the conservative one does. Observed on this recording, not claimed for every market.
#[test]
fn on_the_recorded_run_the_touch_model_fills_no_less_than_trade_through() {
    let filled =
        |m| -> i128 { run(model(m)).0.fills_since(0).iter().map(|f| f.fill.quantity_scaled).sum() };
    assert!(filled(FillModel::Touch) >= filled(FillModel::TradeThrough));
}
