// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Admission gates for desk proposals.

use bijective_venue::profile::IdentifierRejection;
use bijective_venue::profile::{SupportDepth, SupportEntry};
use std::collections::BTreeMap;

use bijective_limits::{Amount, Scope};
use bijective_model::{InstrumentId, Proposal, Side, VenueSymbol};
use bijective_venue::profile::{
    CapabilityDeclaration, DataClass, FundingModel, IdentifierRules, Instrument,
    InstrumentCatalogue, InstrumentKind, InstrumentStatus, OrderTypeSupport, PrecisionRules,
    PriceValidity, ReduceOnly, Transformation, Transport, TransportProfile, VenueProfile,
    declaration::PROFILE_FORMAT,
};

use super::admission::*;

fn instrument(symbol: &str, id: &str, status: InstrumentStatus) -> Instrument {
    Instrument {
        symbol: VenueSymbol::new(symbol),
        id: InstrumentId::new(id),
        kind: InstrumentKind::Perpetual,
        settlement_currency: "usdt".into(),
        quote_currency: "usdt".into(),
        base_currency: None,
        precision: PrecisionRules {
            price_scale: 2,
            quantity_scale: 3,
            price_tick: 10,
            quantity_lot: 1,
            price_validity: PriceValidity::IncrementOnly,
        },
        minimum_size: 1,
        minimum_notional: 1,
        status,
    }
}

fn declaration(venue: &str, instruments: Vec<Instrument>) -> CapabilityDeclaration {
    let mut transports = BTreeMap::new();
    transports.insert(
        Transport::Stream,
        TransportProfile {
            funding: FundingModel::Continuous,
            data_classes: vec![DataClass::TopOfBook],
            max_staleness_ns: Some(5_000_000_000),
        },
    );
    CapabilityDeclaration::from_profile(&VenueProfile {
        format: PROFILE_FORMAT,
        venue: venue.into(),
        verified_at: None,
        transports,
        reduce_only: ReduceOnly::PlacementPrecondition,
        order_types: vec![OrderTypeSupport { name: "limit".into(), native: true }],
        instruments: InstrumentCatalogue::from_instruments(instruments),
        identifier_rules: IdentifierRules { allowed: "abc0123456789".into(), max_len: 16 },
        transformations: vec![],
        support: SupportEntry::declare("venue-a", SupportDepth::Full, true).expect("declared"),
    })
}

/// Two venues listing the same exposure under different symbols.
fn venues() -> BTreeMap<String, CapabilityDeclaration> {
    let mut v = BTreeMap::new();
    v.insert(
        "venue-a".to_string(),
        declaration("venue-a", vec![instrument("aaa", "btc-perp", InstrumentStatus::Trading)]),
    );
    v.insert(
        "venue-b".to_string(),
        declaration("venue-b", vec![instrument("xbt", "btc-perp", InstrumentStatus::Trading)]),
    );
    v
}

fn mandates(preference: &[&str], maximum: i128) -> MandateBook {
    let mut book = MandateBook::new();
    let mut instruments = std::collections::BTreeSet::new();
    instruments.insert(InstrumentId::new("btc-perp"));
    book.declare(
        "alpha",
        Mandate {
            instruments,
            venue_preference: preference.iter().map(|s| (*s).to_string()).collect(),
            maximum_per_order: Amount::from_scaled(maximum),
            price_collar_bps: 500,
        },
    );
    book
}

fn proposal() -> Proposal {
    Proposal {
        desk: "alpha".into(),
        instrument: InstrumentId::new("btc-perp"),
        side: Side::Buy,
        quantity_scaled: 1_500,
        price_scaled: Some(200),
        cap_scaled: None,
        client_id: "abc123".into(),
        decided_at_position: None,
        decided_at_ns: 1,
        venue: None,
        strategy: None,
        terms: Default::default(),
        reduce_only: false,
        parent: None,
        oco: None,
    }
}

#[test]
fn a_proposal_from_a_desk_with_no_mandate_is_refused() {
    let mut orphan = proposal();
    orphan.desk = "unknown".into();

    assert_eq!(
        admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues(), &orphan),
        Err(Refusal::UnknownDesk { desk: "unknown".into() })
    );
}

#[test]
fn an_instrument_the_desk_did_not_declare_is_refused() {
    let mut other = proposal();
    other.instrument = InstrumentId::new("eth-perp");

    assert!(matches!(
        admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues(), &other),
        Err(Refusal::NotMandated { .. })
    ));
}

#[test]
fn a_market_order_without_a_cap_is_refused() {
    let mut market = proposal();
    market.price_scaled = None;
    market.cap_scaled = None;

    assert_eq!(
        admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues(), &market),
        Err(Refusal::NoCapDeclared)
    );
}

#[test]
fn a_market_order_with_a_cap_is_priced_against_it() {
    let mut market = proposal();
    market.price_scaled = None;
    market.cap_scaled = Some(200);

    let admitted =
        admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues(), &market).expect("admit");

    assert_eq!(admitted.spend, Amount::from_scaled(300_000_000));
    assert_eq!(
        admitted.request.price_scaled, None,
        "the cap prices the reservation; it does not become a limit price"
    );
}

#[test]
fn the_spend_is_computed_rather_than_taken_on_trust() {
    let admitted =
        admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues(), &proposal()).expect("admit");

    assert_eq!(admitted.spend, Amount::from_scaled(300_000_000));
}

#[test]
fn an_order_above_the_desks_ceiling_is_refused_rather_than_resized() {
    let outcome = admit_named(&mandates(&["venue-a"], 100_000_000), &venues(), &proposal());

    assert_eq!(
        outcome,
        Err(Refusal::AboveMaximum {
            requested: Amount::from_scaled(300_000_000),
            maximum: Amount::from_scaled(100_000_000)
        })
    );
}

#[test]
fn the_declared_preference_decides_which_venue() {
    let first =
        admit_named(&mandates(&["venue-b", "venue-a"], 1_000_000_000), &venues(), &proposal())
            .expect("admit");

    assert_eq!(first.venue, "venue-b");
    assert_eq!(
        first.request.instrument,
        VenueSymbol::new("xbt"),
        "one identity, and the symbol is whatever that venue calls it"
    );
}

#[test]
fn an_ineligible_venue_falls_through_to_the_next_declared_one() {
    let mut halted = venues();
    halted.insert(
        "venue-b".to_string(),
        declaration("venue-b", vec![instrument("xbt", "btc-perp", InstrumentStatus::Halted)]),
    );

    let admitted =
        admit_named(&mandates(&["venue-b", "venue-a"], 1_000_000_000), &halted, &proposal())
            .expect("admit");

    assert_eq!(admitted.venue, "venue-a");
}

#[test]
fn an_undeclared_venue_is_not_a_fallback() {
    let mut halted = venues();
    halted.insert(
        "venue-a".to_string(),
        declaration("venue-a", vec![instrument("aaa", "btc-perp", InstrumentStatus::Delisted)]),
    );

    let outcome = admit_named(&mandates(&["venue-a"], 1_000_000_000), &halted, &proposal());

    assert!(matches!(outcome, Err(Refusal::NoEligibleVenue { .. })));
}

#[test]
fn a_venue_with_no_execution_side_is_absent_rather_than_refused_separately() {
    let only_b: BTreeMap<_, _> =
        venues().into_iter().filter(|(name, _)| name == "venue-b").collect();

    let outcome = admit_named(&mandates(&["venue-a"], 1_000_000_000), &only_b, &proposal());

    assert!(matches!(outcome, Err(Refusal::NoEligibleVenue { .. })));
}

#[test]
fn a_non_positive_quantity_is_refused() {
    let mut empty = proposal();
    empty.quantity_scaled = 0;

    assert_eq!(
        admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues(), &empty),
        Err(Refusal::NonPositiveQuantity)
    );
}

#[test]
fn the_admitted_order_names_every_scope_it_binds() {
    let admitted =
        admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues(), &proposal()).expect("admit");

    assert_eq!(
        admitted.scopes,
        vec![
            Scope::Portfolio,
            Scope::Desk("alpha".into()),
            Scope::Venue("venue-a".into()),
            Scope::Instrument("btc-perp".into()),
        ],
        "the instrument scope is keyed by identity, so a limit on it binds every venue"
    );
}

/// The strategy that made a proposal is a scope it binds, so a strategy's budget is drawn on.
#[test]
fn a_proposal_naming_its_strategy_binds_the_strategy_scope() {
    let named = Proposal { strategy: Some("trend".into()), ..proposal() };

    let admitted =
        admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues(), &named).expect("admit");

    assert!(admitted.scopes.contains(&Scope::Strategy("trend".into())));
}

/// An order on an instrument with a declared base draws on that asset's budget too.
#[test]
fn an_instrument_with_a_declared_base_binds_its_underlying() {
    let mut btc = instrument("aaa", "btc-perp", InstrumentStatus::Trading);
    btc.base_currency = Some("BTC".into());
    let mut listed = venues();
    listed.insert("venue-a".into(), declaration("venue-a", vec![btc]));

    let admitted =
        admit_named(&mandates(&["venue-a"], 1_000_000_000), &listed, &proposal()).expect("admit");

    assert!(admitted.scopes.contains(&Scope::Underlying("btc".into())));
}

fn instrument_with(symbol: &str, price_tick: i128, quantity_lot: i128) -> Instrument {
    let mut i = instrument(symbol, "btc-perp", InstrumentStatus::Trading);
    i.precision.price_tick = price_tick;
    i.precision.quantity_lot = quantity_lot;
    i
}

fn venues_with(
    instrument: Instrument,
    transformations: Vec<Transformation>,
) -> BTreeMap<String, CapabilityDeclaration> {
    let mut transports = BTreeMap::new();
    transports.insert(
        Transport::Stream,
        TransportProfile {
            funding: FundingModel::Continuous,
            data_classes: vec![DataClass::TopOfBook],
            max_staleness_ns: Some(5_000_000_000),
        },
    );
    let mut v = BTreeMap::new();
    v.insert(
        "venue-a".to_string(),
        CapabilityDeclaration::from_profile(&VenueProfile {
            format: PROFILE_FORMAT,
            venue: "venue-a".into(),
            verified_at: None,
            transports,
            reduce_only: ReduceOnly::PlacementPrecondition,
            order_types: vec![OrderTypeSupport { name: "limit".into(), native: true }],
            instruments: InstrumentCatalogue::from_instruments([instrument]),
            identifier_rules: IdentifierRules { allowed: "abc0123456789".into(), max_len: 16 },
            transformations,
            support: SupportEntry::declare("venue-a", SupportDepth::Full, true).expect("declared"),
        }),
    );
    v
}

#[test]
fn a_price_off_the_venues_increment_is_refused_with_the_increment() {
    let venues = venues_with(instrument_with("aaa", 7, 1), vec![]);

    let outcome = admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues, &proposal());

    assert_eq!(
        outcome,
        Err(Refusal::PriceOffIncrement { value: 200, increment: 7 }),
        "the desk is told what to do, not that it was wrong"
    );
}

#[test]
fn a_quantity_off_the_venues_lot_is_refused() {
    let venues = venues_with(instrument_with("aaa", 10, 400), vec![]);
    let mut priced = proposal();
    priced.price_scaled = Some(200);

    assert_eq!(
        admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues, &priced),
        Err(Refusal::QuantityOffIncrement { value: 1_500, increment: 400 })
    );
}

#[test]
fn a_market_orders_cap_is_not_checked_against_the_price_increment() {
    let venues = venues_with(instrument_with("aaa", 7, 1), vec![]);
    let mut market = proposal();
    market.price_scaled = None;
    market.cap_scaled = Some(200);

    assert!(
        admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues, &market).is_ok(),
        "the cap prices the reservation and never reaches the venue"
    );
}

#[test]
fn a_venue_that_alters_the_price_refuses_the_order() {
    let venues = venues_with(
        instrument_with("aaa", 10, 1),
        vec![Transformation::PriceSignificantFigures { field: "price".into(), figures: 5 }],
    );

    assert_eq!(
        admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues, &proposal()),
        Err(Refusal::WouldBeAltered { field: "price".into() })
    );
}

#[test]
fn an_alteration_to_another_field_does_not_refuse_the_order() {
    let venues = venues_with(
        instrument_with("aaa", 10, 1),
        vec![Transformation::IdentifierNarrowed { field: "client_order_id".into(), to_bits: 64 }],
    );

    assert!(admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues, &proposal()).is_ok());
}

#[test]
fn an_order_below_the_venues_notional_floor_is_refused() {
    let mut listed = instrument("aaa", "btc-perp", InstrumentStatus::Trading);
    listed.minimum_notional = 500;
    let venues = venues_with(listed, vec![]);

    let outcome = admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues, &proposal());

    assert_eq!(
        outcome,
        Err(Refusal::BelowMinimumNotional {
            spend: Amount::from_scaled(300_000_000),
            minimum: Amount::from_scaled(500_000_000)
        }),
        "both figures at the platform's scale, so an operator compares like with like"
    );
}

#[test]
fn an_order_exactly_at_the_floor_is_admitted() {
    let mut listed = instrument("aaa", "btc-perp", InstrumentStatus::Trading);
    listed.minimum_notional = 300;
    let venues = venues_with(listed, vec![]);

    assert!(admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues, &proposal()).is_ok());
}

/// Identifier drives duplicate detection after an unknown outcome.
#[test]
fn an_identifier_the_venue_would_not_accept_is_refused_before_it_is_sent() {
    let mut proposal = proposal();
    proposal.client_id = "bl-\u{e9}\u{e9}\u{e9}".to_string();

    match admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues(), &proposal) {
        Err(Refusal::IdentifierRefused { rejection }) => {
            assert!(matches!(rejection, IdentifierRejection::Character { .. }), "{rejection:?}");
        }
        other => panic!("an identifier the venue refuses was admitted: {other:?}"),
    }
}

#[test]
fn an_identifier_longer_than_the_venue_accepts_is_refused_as_too_long() {
    let mut proposal = proposal();
    proposal.client_id = "a".repeat(200);

    match admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues(), &proposal) {
        Err(Refusal::IdentifierRefused { rejection: IdentifierRejection::TooLong { .. } }) => {}
        other => panic!("{other:?}"),
    }
}

fn with_kind(kind: InstrumentKind) -> BTreeMap<String, CapabilityDeclaration> {
    let mut listed = instrument("aaa", "btc-perp", InstrumentStatus::Trading);
    listed.kind = kind;
    let mut v = BTreeMap::new();
    v.insert("venue-a".to_string(), declaration("venue-a", vec![listed]));
    v
}

#[test]
fn a_spot_and_a_perpetual_instrument_are_admitted() {
    for kind in [InstrumentKind::Spot, InstrumentKind::Perpetual] {
        let outcome =
            admit_named(&mandates(&["venue-a"], 1_000_000_000), &with_kind(kind), &proposal());

        assert!(outcome.is_ok(), "{kind:?} must be admitted, got {outcome:?}");
    }
}

#[test]
fn a_dated_future_and_an_inverse_contract_are_refused_by_kind() {
    for kind in [InstrumentKind::DatedFuture, InstrumentKind::Inverse] {
        let outcome =
            admit_named(&mandates(&["venue-a"], 1_000_000_000), &with_kind(kind), &proposal());

        assert_eq!(
            outcome,
            Err(Refusal::KindNotTradable { instrument: InstrumentId::new("btc-perp"), kind }),
            "{kind:?} has expiry or non-linear settlement the platform does not handle yet",
        );
    }
}

#[test]
fn a_venue_listing_an_unsupported_kind_is_passed_over_for_the_next_preference() {
    let mut listed = with_kind(InstrumentKind::DatedFuture);
    listed.insert(
        "venue-b".to_string(),
        declaration("venue-b", vec![instrument("xbt", "btc-perp", InstrumentStatus::Trading)]),
    );

    let admitted =
        admit_named(&mandates(&["venue-a", "venue-b"], 1_000_000_000), &listed, &proposal())
            .expect("the second preference lists a perpetual");

    assert_eq!(admitted.venue, "venue-b");
}

fn for_venue(venue: &str) -> Proposal {
    Proposal { venue: Some(venue.to_string()), ..proposal() }
}

/// An arbitrage leg or a fee-driven decision is made for one venue. It goes there, not to the first
/// venue in the desk's list.
#[test]
fn a_named_venue_the_desk_routes_to_is_where_the_order_goes() {
    let book = mandates(&["venue-a", "venue-b"], 1_000_000_000);
    let admitted = admit_named(&book, &venues(), &for_venue("venue-b")).expect("admit");
    assert_eq!(admitted.venue, "venue-b");
    assert_eq!(admitted.request.instrument, VenueSymbol::new("xbt"));
    assert!(admitted.scopes.contains(&Scope::Venue("venue-b".to_string())));
}

#[test]
fn a_named_venue_the_desk_does_not_route_to_is_refused_not_redirected() {
    let book = mandates(&["venue-a"], 1_000_000_000);
    let outcome = admit_named(&book, &venues(), &for_venue("venue-b"));
    assert_eq!(outcome, Err(Refusal::VenueNotMandated { venue: "venue-b".to_string() }));
}

/// Without a name the next venue would be tried. With one, a halted instrument there is a refusal.
#[test]
fn a_named_venue_where_the_instrument_is_not_tradable_does_not_fall_through() {
    let mut halted = venues();
    halted.insert(
        "venue-b".to_string(),
        declaration("venue-b", vec![instrument("xbt", "btc-perp", InstrumentStatus::Halted)]),
    );
    let book = mandates(&["venue-b", "venue-a"], 1_000_000_000);
    let outcome = admit_named(&book, &halted, &for_venue("venue-b"));
    assert!(matches!(outcome, Err(Refusal::NoEligibleVenue { .. })), "{outcome:?}");
}

/// An outcome share is admitted like a spot pair, at a price inside its payout bounds. At or
/// beyond them it is refused (decision 0002). Price scale 2: the payout is 100.
#[test]
fn an_outcome_is_admitted_only_between_nothing_and_its_payout() {
    let venues = with_kind(InstrumentKind::Outcome);
    let inside = Proposal { price_scaled: Some(50), ..proposal() };
    assert!(admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues, &inside).is_ok());

    for price in [100, 150] {
        let at = Proposal { price_scaled: Some(price), ..proposal() };
        assert_eq!(
            admit_named(&mandates(&["venue-a"], 1_000_000_000), &venues, &at),
            Err(Refusal::OutsideOutcomeBounds { price, payout: 100 })
        );
    }
}

/// The order identifier the tests send is the proposal's own, so a test sets one in one place.
fn admit_named(
    mandates: &MandateBook,
    venues: &BTreeMap<String, CapabilityDeclaration>,
    proposal: &Proposal,
) -> Result<Admitted, Refusal> {
    crate::desks::admission::admit(mandates, venues, proposal, &proposal.client_id)
}

/// Two accounts at venue-a: a proposal naming the venue goes to the first of them in the desk's
/// order where the instrument is eligible. One naming an account goes to that account only
/// (decision 0008).
#[test]
fn a_proposal_naming_a_venue_is_routed_among_the_desks_accounts_there() {
    let listed = || vec![instrument("aaa", "btc-perp", InstrumentStatus::Trading)];
    let mut accounts = BTreeMap::new();
    accounts.insert("main".to_string(), declaration("venue-a", listed()));
    accounts.insert("client".to_string(), declaration("venue-a", listed()));

    let book = mandates(&["client", "main"], 1_000_000_000);
    let by_venue = admit_named(&book, &accounts, &for_venue("venue-a")).expect("admit");
    assert_eq!(by_venue.venue, "client", "the desk's order decides among the accounts");
    assert!(by_venue.scopes.contains(&Scope::Venue("client".to_string())));

    let by_account = admit_named(&book, &accounts, &for_venue("main")).expect("admit");
    assert_eq!(by_account.venue, "main");

    let only_main = mandates(&["main"], 1_000_000_000);
    assert_eq!(
        admit_named(&only_main, &accounts, &for_venue("client")),
        Err(Refusal::VenueNotMandated { venue: "client".to_string() }),
        "an account the desk does not list is refused, not replaced"
    );
}

/// Post-only needs a price to rest at and a time in force that lets it rest (decision 0009).
#[test]
fn a_post_only_order_that_cannot_rest_is_refused() {
    let book = mandates(&["venue-a"], 1_000_000_000);
    let post = |time_in_force, price| Proposal {
        terms: bijective_model::OrderTerms {
            time_in_force,
            post_only: true,
            stop_price_scaled: None,
            expire_at_ns: None,
        },
        price_scaled: price,
        cap_scaled: Some(2_000_000),
        ..proposal()
    };
    use bijective_model::TimeInForce as T;
    for refused in [post(T::ImmediateOrCancel, Some(100)), post(T::GoodTillCancelled, None)] {
        assert_eq!(admit_named(&book, &venues(), &refused), Err(Refusal::PostOnlyCannotRest));
    }
    let resting = admit_named(&book, &venues(), &post(T::GoodTillCancelled, Some(100)));
    assert!(resting.is_ok_and(|a| a.request.terms.post_only), "terms reach the order");
}

/// A stop off the tick, a non-positive stop and a post-only stop are refused before the venue.
#[test]
fn a_stop_is_checked_as_a_price_is() {
    let book = mandates(&["venue-a"], 1_000_000_000);
    let with_stop = |stop: i128, post_only: bool| {
        let mut p = proposal();
        p.terms.stop_price_scaled = Some(stop);
        p.terms.post_only = post_only;
        admit_named(&book, &venues(), &p)
    };
    assert!(with_stop(300, false).is_ok());
    assert_eq!(with_stop(0, false), Err(Refusal::NonPositiveStop));
    assert_eq!(with_stop(300, true), Err(Refusal::StopCannotBePostOnly));
    assert!(matches!(with_stop(303, false), Err(Refusal::PriceOffIncrement { value: 303, .. })));
}

/// An expiry (decision 0014) needs an order that rests, and reaches the order unchanged.
#[test]
fn an_expiry_needs_a_resting_order() {
    let book = mandates(&["venue-a"], 1_000_000_000);
    let dated = |time_in_force| {
        let mut p = proposal();
        p.terms.time_in_force = time_in_force;
        p.terms.expire_at_ns = Some(5_000);
        admit_named(&book, &venues(), &p)
    };
    use bijective_model::TimeInForce as T;
    assert_eq!(dated(T::ImmediateOrCancel), Err(Refusal::ExpiryCannotRest));
    let resting = dated(T::GoodTillCancelled);
    assert!(resting.is_ok_and(|a| a.request.terms.expire_at_ns == Some(5_000)));
}
