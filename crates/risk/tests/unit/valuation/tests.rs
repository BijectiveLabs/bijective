// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a valuation must refuse to do.

use bijective_model::{HeldPosition, InstrumentId, VenueSymbol};
use bijective_venue::profile::{
    Instrument, InstrumentKind, InstrumentStatus, PrecisionRules, PriceValidity,
};

use super::*;

fn instrument(symbol: &str) -> Instrument {
    Instrument {
        symbol: VenueSymbol::new(symbol),
        id: InstrumentId::new(symbol),
        kind: InstrumentKind::Perpetual,
        settlement_currency: "usdt".into(),
        quote_currency: "usdt".into(),
        // Upper case, as a venue writes it. The underlying is keyed in lower case.
        base_currency: Some(symbol.to_ascii_uppercase()),
        precision: PrecisionRules {
            price_scale: 2,
            quantity_scale: 3,
            price_tick: 10,
            quantity_lot: 1,
            price_validity: PriceValidity::IncrementOnly,
        },
        minimum_size: 1,
        minimum_notional: 1,
        status: InstrumentStatus::Trading,
    }
}

fn catalogue() -> BTreeMap<InstrumentId, Instrument> {
    catalogued([instrument("aaa"), instrument("bbb")])
}

fn catalogued(items: impl IntoIterator<Item = Instrument>) -> BTreeMap<InstrumentId, Instrument> {
    items.into_iter().map(|i| (i.id.clone(), i)).collect()
}

fn settling_in(symbol: &str, currency: &str) -> Instrument {
    let mut i = instrument(symbol);
    i.settlement_currency = currency.into();
    i
}

fn held(id: &str, quantity_scaled: i128) -> HeldPosition {
    HeldPosition {
        venue: "venue-a".to_string(),
        instrument: InstrumentId::new(id),
        quantity_scaled,
        entry_price_scaled: 200,
    }
}

fn marked(pairs: &[(&str, i128, i128)]) -> BTreeMap<InstrumentId, Mark> {
    pairs
        .iter()
        .map(|(s, price, at)| {
            (InstrumentId::new(*s), Mark { price_scaled: *price, observed_at_ns: *at })
        })
        .collect()
}

fn figure(snapshot: &ExposureSnapshot, kind: LimitKind) -> Option<Threshold> {
    snapshot
        .figures
        .iter()
        .find(|(scope, k, _)| *scope == Scope::Portfolio && *k == kind)
        .map(|(_, _, v)| *v)
}

#[test]
fn an_empty_book_values_to_nothing_held() {
    let valued =
        value(&[], &Pricing::single(&BTreeMap::new(), &catalogue()), &Venues::default(), 100);

    assert!(valued.unvalued.is_empty());
    assert_eq!(
        figure(&valued.snapshot, LimitKind::GrossExposure),
        Some(Threshold::Money(Amount::from_scaled(0))),
        "holding nothing is not the same as not knowing what is held"
    );
}

/// Gross adds magnitudes. Net adds signs. A hedged book is flat in one and not the other.
#[test]
fn a_long_and_a_short_cancel_in_net_and_add_in_gross() {
    let valued = value(
        &[held("aaa", 1_500), held("bbb", -1_500)],
        &Pricing::single(&marked(&[("aaa", 200, 100), ("bbb", 200, 100)]), &catalogue()),
        &Venues::default(),
        100,
    );

    assert!(valued.unvalued.is_empty());
    assert_eq!(
        figure(&valued.snapshot, LimitKind::GrossExposure),
        Some(Threshold::Money(Amount::from_scaled(600_000_000))),
        "3.00 each way"
    );
    assert_eq!(
        figure(&valued.snapshot, LimitKind::NetExposure),
        Some(Threshold::Money(Amount::from_scaled(0)))
    );
}

/// Wrong in the direction that permits more trading. Risk limits reads the absent figure as unjudged and halts.
#[test]
fn a_position_with_no_price_empties_the_snapshot_rather_than_shrinking_it() {
    let valued = value(
        &[held("aaa", 1_500), held("bbb", 1_500)],
        &Pricing::single(&marked(&[("aaa", 200, 100)]), &catalogue()),
        &Venues::default(),
        100,
    );

    assert_eq!(valued.unvalued, vec![Unvalued::NoMark { instrument: InstrumentId::new("bbb") }]);
    assert!(
        valued.snapshot.figures.is_empty(),
        "a partial valuation must not be reported as a figure"
    );
}

#[test]
fn a_position_the_catalogue_does_not_carry_is_unvalued() {
    let valued = value(
        &[held("zzz", 1_500)],
        &Pricing::single(&marked(&[("zzz", 200, 100)]), &catalogue()),
        &Venues::default(),
        100,
    );

    assert_eq!(
        valued.unvalued,
        vec![Unvalued::NotCatalogued { instrument: InstrumentId::new("zzz") }]
    );
}

/// Taking the newest would let one live price make a stale book look current.
#[test]
fn the_valuation_is_only_as_fresh_as_its_stalest_ingredient() {
    let valued = value(
        &[held("aaa", 1_000), held("bbb", 1_000)],
        &Pricing::single(&marked(&[("aaa", 200, 900), ("bbb", 200, 400)]), &catalogue()),
        &Venues::default(),
        1_000,
    );

    assert_eq!(valued.snapshot.observed_at_ns, 400);
}

#[test]
fn stale_positions_make_a_stale_valuation() {
    let valued = value(
        &[held("aaa", 1_000)],
        &Pricing::single(&marked(&[("aaa", 200, 9_000)]), &catalogue()),
        &Venues::default(),
        50,
    );

    assert_eq!(valued.snapshot.observed_at_ns, 50);
}

#[test]
fn an_unrepresentable_value_is_reported_rather_than_wrapped() {
    let valued = value(
        &[held("aaa", i128::MAX)],
        &Pricing::single(&marked(&[("aaa", 1_000, 100)]), &catalogue()),
        &Venues::default(),
        100,
    );

    assert!(matches!(valued.unvalued.first(), Some(Unvalued::Arithmetic { .. })));
    assert!(valued.snapshot.figures.is_empty());
}

/// A sum across settlement currencies is not money, and the total would feed every risk limits limit.
#[test]
fn a_book_spanning_two_settlement_currencies_produces_no_figure() {
    let catalogue = catalogued([settling_in("aaa", "usdt"), settling_in("bbb", "btc")]);

    let v = value(
        &[held("aaa", 1_000), held("bbb", 1_000)],
        &Pricing::single(&marked(&[("aaa", 200, 10), ("bbb", 200, 10)]), &catalogue),
        &Venues::default(),
        10,
    );

    assert!(
        v.snapshot.figures.is_empty(),
        "a figure was produced from two currencies: {:?}",
        v.snapshot.figures
    );
    assert!(
        v.unvalued.iter().any(|u| matches!(
            u,
            Unvalued::ForeignSettlement { settles_in, book_settles_in, .. }
                if settles_in == "btc" && book_settles_in == "usdt"
        )),
        "the refusal did not name the two currencies: {:?}",
        v.unvalued
    );
}

#[test]
fn a_book_settling_in_one_currency_still_values() {
    let v = value(
        &[held("aaa", 1_000), held("bbb", 1_000)],
        &Pricing::single(&marked(&[("aaa", 200, 10), ("bbb", 200, 10)]), &catalogue()),
        &Venues::default(),
        10,
    );

    assert!(v.unvalued.is_empty(), "{:?}", v.unvalued);

    // A figure per scope: the book, each venue, each instrument.
    let has = |scope: Scope, kind: LimitKind| {
        v.snapshot.figures.iter().any(|(s, k, _)| *s == scope && *k == kind)
    };
    assert!(has(Scope::Portfolio, LimitKind::GrossExposure));
    assert!(has(Scope::Venue("venue-a".to_string()), LimitKind::NetExposure));
    assert!(has(Scope::Instrument("aaa".to_string()), LimitKind::GrossExposure));
    assert!(
        !has(Scope::Desk("alpha".to_string()), LimitKind::GrossExposure),
        "a desk figure was produced from venue positions, which do not say which desk opened them"
    );
}

/// Inverse value is not quantity times price, and the error grows in the direction price moved.
#[test]
fn an_inverse_contract_is_refused_rather_than_valued_linearly() {
    let mut inverse = instrument("bbb");
    inverse.kind = InstrumentKind::Inverse;
    let catalogue = catalogued([instrument("aaa"), inverse]);

    let v = value(
        &[held("aaa", 1_000), held("bbb", 1_000)],
        &Pricing::single(&marked(&[("aaa", 200, 10), ("bbb", 200, 10)]), &catalogue),
        &Venues::default(),
        10,
    );

    assert!(v.snapshot.figures.is_empty(), "{:?}", v.snapshot.figures);
    assert!(
        v.unvalued
            .iter()
            .any(|u| matches!(u, Unvalued::UnsupportedKind { kind: InstrumentKind::Inverse, .. })),
        "{:?}",
        v.unvalued
    );
}

/// A dated future has an expiry and a settlement this valuation does not model.
#[test]
fn a_dated_future_is_refused_rather_than_valued_as_a_perpetual() {
    let mut future = instrument("bbb");
    future.kind = InstrumentKind::DatedFuture;
    let catalogue = catalogued([instrument("aaa"), future]);

    let v = value(
        &[held("aaa", 1_000), held("bbb", 1_000)],
        &Pricing::single(&marked(&[("aaa", 200, 10), ("bbb", 200, 10)]), &catalogue),
        &Venues::default(),
        10,
    );

    assert!(v.snapshot.figures.is_empty(), "{:?}", v.snapshot.figures);
    assert!(
        v.unvalued.iter().any(|u| matches!(
            u,
            Unvalued::UnsupportedKind { kind: InstrumentKind::DatedFuture, .. }
        )),
        "{:?}",
        v.unvalued
    );
}

/// A spot holding is valued at quantity times price, like any linear position.
#[test]
fn a_spot_holding_is_valued_as_quantity_times_price() {
    let mut spot = instrument("bbb");
    spot.kind = InstrumentKind::Spot;
    let catalogue = catalogued([spot]);

    let v = value(
        &[held("bbb", 1_000)],
        &Pricing::single(&marked(&[("bbb", 200, 10)]), &catalogue),
        &Venues::default(),
        10,
    );

    assert!(v.unvalued.is_empty(), "{:?}", v.unvalued);
    assert!(!v.snapshot.figures.is_empty());
}

/// Keyed by platform identity, so two venues sharing a symbol cannot borrow each other's mark.
#[test]
fn one_symbol_on_two_venues_is_two_identities_and_two_prices() {
    let mut first = instrument("aaa");
    first.id = InstrumentId::new("venue-a.btcusdt");
    first.symbol = VenueSymbol::new("btcusdt");

    let mut second = instrument("bbb");
    second.id = InstrumentId::new("venue-b.btcusdt");
    second.symbol = VenueSymbol::new("btcusdt");

    let catalogue = catalogued([first, second]);

    let valued = value(
        &[held("venue-a.btcusdt", 1_000), held("venue-b.btcusdt", 1_000)],
        &Pricing::single(
            &marked(&[("venue-a.btcusdt", 200, 10), ("venue-b.btcusdt", 400, 10)]),
            &catalogue,
        ),
        &Venues::default(),
        10,
    );

    assert!(valued.unvalued.is_empty(), "{:?}", valued.unvalued);

    // 200 against 400 on the same quantity: equal legs would mean both took one mark.
    let gross = valued
        .snapshot
        .figures
        .iter()
        .find(|(scope, kind, _)| *scope == Scope::Portfolio && *kind == LimitKind::GrossExposure)
        .map(|(_, _, t)| *t)
        .expect("gross");
    let single = value(
        &[held("venue-a.btcusdt", 1_000)],
        &Pricing::single(&marked(&[("venue-a.btcusdt", 200, 10)]), &catalogue),
        &Venues::default(),
        10,
    );
    let one_leg = single
        .snapshot
        .figures
        .iter()
        .find(|(scope, kind, _)| *scope == Scope::Portfolio && *kind == LimitKind::GrossExposure)
        .map(|(_, _, t)| *t)
        .expect("gross");

    assert_ne!(gross, one_leg, "the second venue's leg was not counted separately");
}

/// What is held at a withdrawn venue is unknown, not nothing.
#[test]
fn a_withdrawn_venue_leaves_no_figure_even_when_the_others_answered() {
    let valued = value(
        &[held("aaa", 1_000)],
        &Pricing::single(&marked(&[("aaa", 200, 10)]), &catalogue()),
        &Venues { read: &[], withdrawn: &["venue-b".to_string()] },
        10,
    );

    assert!(
        valued.snapshot.figures.is_empty(),
        "a total was produced while a venue was withdrawn: {:?}",
        valued.snapshot.figures
    );
    assert!(
        valued
            .unvalued
            .iter()
            .any(|u| matches!(u, Unvalued::VenueWithdrawn { venue } if venue == "venue-b")),
        "{:?}",
        valued.unvalued
    );
}

/// `MEASURED_LEVELS` must match what is emitted: drift lets through a limit nothing can judge, and risk limits halts.
#[test]
fn every_level_measured_is_declared_and_every_level_declared_is_measured() {
    let mut here = held("aaa", 1_000);
    here.venue = "venue-a".into();
    let mut there = held("bbb", -500);
    there.venue = "venue-b".into();

    let valued = value(
        &[here, there],
        &Pricing::single(&marked(&[("aaa", 100, 10), ("bbb", 200, 10)]), &catalogue()),
        &Venues::default(),
        10,
    );
    assert!(valued.unvalued.is_empty(), "{:?}", valued.unvalued);

    let ordered = |mut levels: Vec<ScopeLevel>| {
        levels.sort_by_key(|l| format!("{l:?}"));
        levels.dedup();
        levels
    };

    let (marks, listed) = (marked(&[("aaa", 100, 10), ("bbb", 200, 10)]), catalogue());
    let pricing = Pricing::single(&marks, &listed);
    let placed = [attribution::Placed {
        owner: Some(bijective_model::Owner {
            desk: "alpha".into(),
            strategy: Some("trend".into()),
        }),
        venue: "venue-a".into(),
        instrument: InstrumentId::new("aaa"),
        filled_scaled: 1_000,
        open_buy_scaled: 0,
        open_sell_scaled: 0,
    }];
    let by_owner =
        attribution::attributed(&[], &placed, &mut currency::Conversion::new(pricing, None), &[])
            .expect("attributed");

    let measured = ordered(
        valued
            .snapshot
            .figures
            .iter()
            .chain(&by_owner.figures)
            .map(|(scope, _, _)| scope.level())
            .collect(),
    );

    assert_eq!(
        measured,
        ordered(MEASURED_LEVELS.to_vec()),
        "the levels this build measures are not the levels it tells an accepting gate it measures"
    );
}

/// A realised figure is at the instrument's price scale plus quantity scale. The loss is the day's
/// result turned over, measured from where the day opened.
#[test]
fn a_daily_loss_is_the_day_s_result_turned_over() {
    let i = instrument("aaa");
    // 300 at scale 2 + 3 is 0.00300.
    assert_eq!(realized_value(300, &i), Ok(Amount::from_scaled(300_000)));

    // Realised -1.00 today. The book was at +2.00 unrealised at the open and is at +0.50 now.
    let loss = daily_loss(
        Amount::from_scaled(-100_000_000),
        Amount::from_scaled(50_000_000),
        Amount::from_scaled(200_000_000),
    );
    assert_eq!(
        loss,
        Ok((
            Scope::Portfolio,
            LimitKind::DailyLoss,
            Threshold::Money(Amount::from_scaled(250_000_000))
        ))
    );

    let gain = daily_loss(Amount::from_scaled(10), Amount::from_scaled(0), Amount::from_scaled(0));
    assert_eq!(
        gain,
        Ok((Scope::Portfolio, LimitKind::DailyLoss, Threshold::Money(Amount::from_scaled(-10)))),
        "a gain is a negative loss, which no maximum is below"
    );
}

/// Profit not yet realised: what the mark has moved since entry, signed by the position's side.
#[test]
fn unrealised_profit_is_the_move_since_entry_signed_by_side() {
    let marks = marked(&[("aaa", 250, 10), ("bbb", 150, 10)]);
    let book = [held("aaa", 1_000), held("bbb", -2_000)];
    let valuation = value(&book, &Pricing::single(&marks, &catalogue()), &Venues::default(), 10);
    // aaa: +0.50 on one unit long. bbb: +1.00 on two units short.
    assert_eq!(valuation.unrealized, Some(Amount::from_scaled(150_000_000)));

    let losing = marked(&[("aaa", 150, 10), ("bbb", 250, 10)]);
    let valuation = value(&book, &Pricing::single(&losing, &catalogue()), &Venues::default(), 10);
    assert_eq!(valuation.unrealized, Some(Amount::from_scaled(-150_000_000)));
}

/// A book that could not be valued has no profit figure: a gain computed on part of it would hide a loss.
#[test]
fn an_unvalued_book_has_no_unrealised_figure() {
    let marks = marked(&[("aaa", 250, 10)]);
    let book = [held("aaa", 1_000), held("bbb", 1_000)];
    assert_eq!(
        value(&book, &Pricing::single(&marks, &catalogue()), &Venues::default(), 10).unrealized,
        None
    );
    assert_eq!(
        value(&[], &Pricing::single(&marked(&[]), &catalogue()), &Venues::default(), 10).unrealized,
        Some(Amount::from_scaled(0))
    );
    let withdrawn = value(
        &[],
        &Pricing::single(&marked(&[]), &catalogue()),
        &Venues { read: &[], withdrawn: &["venue-a".to_string()] },
        10,
    );
    assert_eq!(withdrawn.unrealized, None);
}

fn underlying(valued: &Valuation, asset: &str, kind: LimitKind) -> Option<Threshold> {
    valued
        .snapshot
        .figures
        .iter()
        .find(|(s, k, _)| *s == Scope::Underlying(asset.into()) && *k == kind)
        .map(|(_, _, v)| *v)
}

/// One asset on two venues is one underlying: a long on each adds, a long and a short net.
#[test]
fn an_underlying_sums_one_asset_across_venues() {
    let mut there = HeldPosition {
        venue: "venue-b".into(),
        instrument: InstrumentId::new("aaa-b"),
        quantity_scaled: -500,
        entry_price_scaled: 200,
    };
    let mut on_b = instrument("aaa-b");
    on_b.base_currency = Some("AAA".into());
    let listed = catalogued([instrument("aaa"), on_b]);
    let valued = value(
        &[held("aaa", 1_000), there.clone()],
        &Pricing::single(&marked(&[("aaa", 200, 10), ("aaa-b", 200, 10)]), &listed),
        &Venues::default(),
        10,
    );

    // 1.000 at 2.00 and -0.500 at 2.00: gross 3.00, net 1.00.
    assert_eq!(
        underlying(&valued, "aaa", LimitKind::GrossExposure),
        Some(Threshold::Money(Amount::from_scaled(300_000_000)))
    );
    assert_eq!(
        underlying(&valued, "aaa", LimitKind::NetExposure),
        Some(Threshold::Money(Amount::from_scaled(100_000_000)))
    );

    there.quantity_scaled = 500;
    let both_long = value(
        &[held("aaa", 1_000), there],
        &Pricing::single(&marked(&[("aaa", 200, 10), ("aaa-b", 200, 10)]), &listed),
        &Venues::default(),
        10,
    );
    assert_eq!(
        underlying(&both_long, "aaa", LimitKind::NetExposure),
        Some(Threshold::Money(Amount::from_scaled(300_000_000)))
    );
}

/// An unknown base could be any asset, so no underlying figure stands. The others still do.
#[test]
fn a_position_without_a_declared_base_leaves_every_underlying_unmeasured() {
    let mut unknown = instrument("bbb");
    unknown.base_currency = None;
    let valued = value(
        &[held("aaa", 1_000), held("bbb", 1_000)],
        &Pricing::single(
            &marked(&[("aaa", 200, 10), ("bbb", 200, 10)]),
            &catalogued([instrument("aaa"), unknown]),
        ),
        &Venues::default(),
        10,
    );

    assert!(valued.unvalued.is_empty());
    assert_eq!(underlying(&valued, "aaa", LimitKind::GrossExposure), None);
    assert!(
        figure(&valued.snapshot, LimitKind::GrossExposure).is_some(),
        "the portfolio is still valued"
    );
}

/// A flat book is known to hold nothing at every venue read, instrument listed and underlying
/// named: each has a zero figure, so a limit declared there is judged and a node can start flat.
#[test]
fn a_flat_book_has_a_zero_figure_at_every_level_it_lists() {
    let read = ["venue-a".to_string()];
    let venues = Venues { read: &read, withdrawn: &[] };

    let valued = value(&[], &Pricing::single(&marked(&[]), &catalogue()), &venues, 10);

    let zero = Some(Threshold::Money(Amount::from_scaled(0)));
    let at = |scope: Scope| {
        valued
            .snapshot
            .figures
            .iter()
            .find(|(s, k, _)| *s == scope && *k == LimitKind::GrossExposure)
            .map(|(_, _, v)| *v)
    };
    assert_eq!(at(Scope::Venue("venue-a".into())), zero);
    assert_eq!(at(Scope::Instrument("aaa".into())), zero);
    assert_eq!(at(Scope::Underlying("bbb".into())), zero);
    assert_eq!(at(Scope::Underlying("zzz".into())), None, "an asset no venue lists has none");
}
