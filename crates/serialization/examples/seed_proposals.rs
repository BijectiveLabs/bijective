// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Put proposals on the stream the Control Engine reads, for a drill that has no strategy running.
//! With `--with-unreadable`, one entry carries an unregistered class: it must be refused and recorded, never skipped.
//! With `--backtest`, a buy and a sell that cross the seeded book go on the backtest's stream,
//! each stating the market position it was decided at (decision 0015).

fn proposal(
    instrument: &str,
    id: &str,
    side: bijective_model::Side,
    quantity: i128,
    price: i128,
    at: Option<u64>,
) -> bijective_model::Proposal {
    bijective_model::Proposal {
        desk: "alpha".into(),
        instrument: bijective_model::InstrumentId::new(instrument),
        side,
        quantity_scaled: quantity,
        price_scaled: Some(price),
        cap_scaled: None,
        client_id: id.into(),
        decided_at_position: at,
        decided_at_ns: 0,
        venue: None,
        strategy: None,
        terms: Default::default(),
        reduce_only: false,
        parent: None,
        oco: None,
    }
}

fn main() {
    let root = std::env::args().nth(1).expect("store root");
    // The identity, not a venue symbol: a symbol is refused as unmandated.
    let instrument =
        std::env::args().nth(2).filter(|a| !a.starts_with("--")).expect("instrument identity");
    let unreadable = std::env::args().any(|a| a == "--with-unreadable");
    let backtest = std::env::args().any(|a| a == "--backtest");
    let stream = if backtest {
        bijective_serialization::signal::node::BACKTEST.proposals
    } else {
        "proposals"
    };
    let mut w =
        bijective_event_store::store::StreamWriter::open(std::path::Path::new(&root), stream)
            .expect("open");

    let proposals = if backtest {
        // Above the ask and below the bid of `seed_market`'s book, so both take liquidity.
        vec![
            proposal(&instrument, "buy", bijective_model::Side::Buy, 1, 2_660_000, Some(1)),
            proposal(&instrument, "sell", bijective_model::Side::Sell, 1, 2_650_000, Some(3)),
        ]
    } else {
        vec![
            proposal(&instrument, "ord1", bijective_model::Side::Buy, 1_500, 200, None),
            proposal(&instrument, "ord2", bijective_model::Side::Buy, 2_000, 200, None),
        ]
    };
    for p in &proposals {
        w.append(&bijective_serialization::signal::encode(p).expect("encode")).expect("append");
    }

    if unreadable {
        // A class id no registry entry claims, as a Strategy Engine from a newer build would write.
        w.append(&[0xff, 0xff, 0x00, 0x01]).expect("append");
        println!("seeded 2 proposals and 1 unreadable entry on {stream}");
    } else {
        println!("seeded 2 proposals on {stream}");
    }
}
