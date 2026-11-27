// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The contract's own gates, and the conformance suite proven by rejection.

use crate::profile::{SupportDepth, SupportEntry};
use std::collections::BTreeMap;

use crate::normalise::{Attempted, Rejection, Unknown, VenueOutcome};
use crate::profile::{
    CapabilityDeclaration, DataClass, IdentifierRules, Instrument, InstrumentCatalogue,
    InstrumentId, InstrumentKind, InstrumentStatus, OrderTypeSupport, PrecisionRules,
    PriceValidity, ReduceOnly, Transport, TransportProfile, VenueProfile, VenueSymbol,
    declaration::PROFILE_FORMAT,
};

use super::conformance::{CheckOutcome, check_data, check_execution, is_releasable};
use super::*;

/// Mints a clearance the same way the order path does.
fn cleared() -> Clearance {
    Clearance::granted(0, 0, true, 1).expect("nothing breached, nothing unjudged, fresh")
}
use crate::contract::conformance::DeclarationCheck;

fn declaration() -> CapabilityDeclaration {
    declaration_serving(vec![DataClass::TopOfBook])
}

/// The one test venue, declaring the given classes. Shared with the record contract's tests.
pub(crate) fn declaration_serving(data_classes: Vec<DataClass>) -> CapabilityDeclaration {
    let mut transports = BTreeMap::new();
    transports.insert(
        Transport::Stream,
        TransportProfile {
            funding: crate::profile::FundingModel::Continuous,
            data_classes,
            max_staleness_ns: Some(5_000_000_000),
        },
    );
    let p = VenueProfile {
        format: PROFILE_FORMAT,
        venue: "venue-a".into(),
        verified_at: None,
        transports,
        reduce_only: ReduceOnly::PlacementPrecondition,
        order_types: vec![OrderTypeSupport { name: "limit".into(), native: true }],
        instruments: InstrumentCatalogue::from_instruments([Instrument {
            symbol: VenueSymbol::new("aaa"),
            id: InstrumentId::new("aaa"),
            kind: InstrumentKind::Perpetual,
            settlement_currency: "usdt".into(),
            quote_currency: "usdt".into(),
            base_currency: None,
            precision: PrecisionRules {
                price_scale: 2,
                quantity_scale: 3,
                price_tick: 5,
                quantity_lot: 10,
                price_validity: PriceValidity::IncrementOnly,
            },
            minimum_size: 1,
            minimum_notional: 1,
            status: InstrumentStatus::Trading,
        }]),
        identifier_rules: IdentifierRules { allowed: "abc0123456789".into(), max_len: 16 },
        transformations: vec![],
        support: SupportEntry::declare("venue-a", SupportDepth::Full, true).expect("declared"),
    };
    CapabilityDeclaration::from_profile(&p)
}

fn order() -> OrderRequest {
    OrderRequest {
        client_id: "abc123".into(),
        instrument: "aaa".into(),
        side: Side::Buy,
        quantity_scaled: 10,
        price_scaled: Some(2_654_375),
        reduce_only: false,
        terms: Default::default(),
    }
}

struct GoodAdapter {
    declaration: CapabilityDeclaration,
    state: ConnectionState,
    seen: Vec<String>,
}

impl GoodAdapter {
    fn new() -> Self {
        Self { declaration: declaration(), state: ConnectionState::Disconnected, seen: Vec::new() }
    }
}

impl VenueData for GoodAdapter {
    fn declaration(&self) -> &CapabilityDeclaration {
        &self.declaration
    }

    fn connect(&mut self, now_ns: i128) -> Answer<'_, Result<ConnectionState, ConnectError>> {
        Box::pin(async move {
            self.state = ConnectionState::Connected { since: Moment::Observed(now_ns) };
            Ok(self.state.clone())
        })
    }

    fn disconnect(&mut self) -> Answer<'_, ()> {
        Box::pin(async move {
            self.state = ConnectionState::Disconnected;
        })
    }

    fn state(&self) -> ConnectionState {
        self.state.clone()
    }

    fn reconnect(&mut self, now_ns: i128) -> Answer<'_, Result<Reconnected, ConnectError>> {
        Box::pin(async move {
            self.state = ConnectionState::Connected { since: Moment::Observed(now_ns) };
            Ok(Reconnected {
                state: self.state.clone(),
                continuity: Continuity::Gap { from_seq: 10, to_seq: 14 },
            })
        })
    }

    fn subscribe<'a>(
        &'a mut self,
        class: DataClass,
        _instrument: &'a VenueSymbol,
        now_ns: i128,
    ) -> Answer<'a, Result<SubscriptionAck, SubscribeError>> {
        Box::pin(async move {
            if !self.declaration.serves(class) {
                return Err(SubscribeError::NotServed);
            }
            Ok(SubscriptionAck::Confirmed { at_ns: now_ns })
        })
    }
}

impl VenueExecution for GoodAdapter {
    fn declaration(&self) -> &CapabilityDeclaration {
        &self.declaration
    }

    fn submit<'a>(
        &'a mut self,
        _clearance: &'a Clearance,
        order: &'a OrderRequest,
        _now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        Box::pin(async move {
            if self.seen.contains(&order.client_id) {
                return VenueOutcome::Rejected(Rejection::Duplicate);
            }
            self.seen.push(order.client_id.clone());
            VenueOutcome::Accepted
        })
    }

    fn cancel<'a>(
        &'a mut self,
        _instrument: &'a VenueSymbol,
        _client_id: &'a str,
        _now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        Box::pin(async move { VenueOutcome::Accepted })
    }

    fn account(&mut self, now_ns: i128) -> Answer<'_, Result<AccountSnapshot, ConnectError>> {
        Box::pin(async move {
            Ok(AccountSnapshot { observed_at_ns: now_ns, positions: vec![], margin: None })
        })
    }
}

struct BadAdapter {
    declaration: CapabilityDeclaration,
}

impl VenueData for BadAdapter {
    fn declaration(&self) -> &CapabilityDeclaration {
        &self.declaration
    }

    fn connect(&mut self, _now_ns: i128) -> Answer<'_, Result<ConnectionState, ConnectError>> {
        Box::pin(async move { Ok(ConnectionState::Connecting) })
    }

    fn disconnect(&mut self) -> Answer<'_, ()> {
        Box::pin(async move {})
    }

    fn state(&self) -> ConnectionState {
        ConnectionState::Connecting
    }

    /// Claims an unbroken stream while not connected - the unreported gap the adapter contract forbids.
    fn reconnect(&mut self, _now_ns: i128) -> Answer<'_, Result<Reconnected, ConnectError>> {
        Box::pin(async move {
            Ok(Reconnected { state: ConnectionState::Connecting, continuity: Continuity::Unbroken })
        })
    }

    /// Reports a subscription it never had confirmed.
    fn subscribe<'a>(
        &'a mut self,
        _class: DataClass,
        _instrument: &'a VenueSymbol,
        now_ns: i128,
    ) -> Answer<'a, Result<SubscriptionAck, SubscribeError>> {
        Box::pin(async move { Ok(SubscriptionAck::Unconfirmed { requested_at_ns: now_ns }) })
    }
}

impl VenueExecution for BadAdapter {
    fn declaration(&self) -> &CapabilityDeclaration {
        &self.declaration
    }

    /// Accepts the same client identifier twice, which invents an order.
    fn submit<'a>(
        &'a mut self,
        _clearance: &'a Clearance,
        _order: &'a OrderRequest,
        _now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        Box::pin(async move { VenueOutcome::Accepted })
    }

    fn cancel<'a>(
        &'a mut self,
        _instrument: &'a VenueSymbol,
        _client_id: &'a str,
        _now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        Box::pin(async move { VenueOutcome::Accepted })
    }

    fn account(&mut self, now_ns: i128) -> Answer<'_, Result<AccountSnapshot, ConnectError>> {
        Box::pin(async move {
            Ok(AccountSnapshot { observed_at_ns: now_ns, positions: vec![], margin: None })
        })
    }
}

/// The confirmed-subscription rule, structurally: sending is not attaching.
#[test]
fn an_unconfirmed_subscription_is_not_subscribed() {
    assert!(SubscriptionAck::Confirmed { at_ns: 1 }.is_attached());
    assert!(!SubscriptionAck::Unconfirmed { requested_at_ns: 1 }.is_attached());
}

/// Attached and stale is worse than down: down is visible, stale looks like calm.
#[test]
fn a_degraded_connection_is_not_usable() {
    assert!(ConnectionState::Connected { since: Moment::Observed(1) }.is_usable());
    for reason in [
        DegradedReason::Backlogged { queued: 500 },
        DegradedReason::Stale { last_message: Moment::Observed(1) },
        DegradedReason::SubscriptionsIncomplete { attached: 1, requested: 3 },
    ] {
        assert!(!ConnectionState::Degraded { since: Moment::Observed(1), reason }.is_usable());
    }
    assert!(!ConnectionState::Disconnected.is_usable());
}

/// Not knowing a gap's extent is a different fact from knowing it, and it is the worse one.
#[test]
fn a_gap_of_unknown_extent_is_distinguishable() {
    assert!(Continuity::Unbroken.is_unbroken());
    assert!(Continuity::Unbroken.extent_is_known());

    let bounded = Continuity::Gap { from_seq: 4, to_seq: 9 };
    assert!(!bounded.is_unbroken());
    assert!(bounded.extent_is_known());

    assert!(!Continuity::GapOfUnknownExtent.is_unbroken());
    assert!(!Continuity::GapOfUnknownExtent.extent_is_known());
}

#[tokio::test]
async fn a_well_behaved_adapter_passes_the_data_checks() {
    let mut a = GoodAdapter::new();
    let results =
        check_data(&mut a, &VenueSymbol::new("aaa"), 1_000, &DeclarationCheck::Unverified).await;
    let failed: Vec<_> =
        results.iter().filter(|r| matches!(r.outcome, CheckOutcome::Failed { .. })).collect();
    assert!(failed.is_empty(), "unexpected failures: {failed:?}");
}

/// Proven by rejection: the suite catches a never-confirmed subscription and a false unbroken claim.
#[tokio::test]
async fn the_suite_refuses_an_adapter_that_hides_a_gap_or_a_subscription() {
    let mut bad = BadAdapter { declaration: declaration() };
    let results =
        check_data(&mut bad, &VenueSymbol::new("aaa"), 1_000, &DeclarationCheck::Unverified).await;

    let names: Vec<_> = results
        .iter()
        .filter(|r| matches!(r.outcome, CheckOutcome::Failed { .. }))
        .map(|r| r.name)
        .collect();
    assert!(names.contains(&"subscribe-acknowledged"), "{names:?}");
    assert!(names.contains(&"reconnect-declares-continuity"), "{names:?}");
}

#[tokio::test]
async fn the_suite_refuses_an_adapter_that_accepts_the_same_identifier_twice() {
    let mut bad = BadAdapter { declaration: declaration() };
    let results = check_execution(&mut bad, &cleared(), &order(), 1_000).await;
    let failed: Vec<_> = results
        .iter()
        .filter(|r| matches!(r.outcome, CheckOutcome::Failed { .. }))
        .map(|r| r.name)
        .collect();
    assert!(failed.contains(&"resubmission-is-idempotent"), "{failed:?}");
}

#[tokio::test]
async fn a_well_behaved_adapter_answers_duplicate_on_resubmission() {
    let mut a = GoodAdapter::new();
    assert_eq!(a.submit(&cleared(), &order(), 1).await, VenueOutcome::Accepted);
    assert_eq!(
        a.submit(&cleared(), &order(), 2).await,
        VenueOutcome::Rejected(Rejection::Duplicate)
    );
}

/// A check that needs a live venue is not a pass.
#[tokio::test]
async fn requiring_a_live_venue_blocks_release() {
    let mut a = GoodAdapter::new();
    let results =
        check_data(&mut a, &VenueSymbol::new("aaa"), 1_000, &DeclarationCheck::Unverified).await;
    assert!(
        results.iter().any(|r| r.outcome == CheckOutcome::RequiresLiveVenue),
        "the suite claims to cover everything without a venue"
    );
    assert!(!is_releasable(&results), "an adapter was releasable while a check had not run");
}

#[tokio::test]
async fn an_empty_result_set_is_not_releasable() {
    assert!(!is_releasable(&[]));
}

/// An unknown outcome never reports the order as not having happened.
#[tokio::test]
async fn the_execution_checks_hold_the_unknown_distinction() {
    let o = VenueOutcome::Unknown(Unknown::Timeout);
    assert!(!o.is_known_not_to_have_happened());
    assert!(o.needs_reconciliation(Attempted::Submit));
}

#[tokio::test]
async fn a_verified_declaration_passes_the_check_it_used_to_report_as_unproven() {
    let mut adapter = GoodAdapter::new();
    let results =
        check_data(&mut adapter, &VenueSymbol::new("aaa"), 1_000, &DeclarationCheck::Matches).await;

    let check = results
        .iter()
        .find(|r| r.name == "declaration-matches-live-behaviour")
        .expect("the check is present");
    assert!(check.is_pass());
    assert!(is_releasable(&results), "with nothing unproven, the suite may release");
}

/// A divergence is a failure, not an absent result. The suite must not release over a capital-relevant one.
#[tokio::test]
async fn a_diverging_declaration_fails_the_suite() {
    let mut adapter = GoodAdapter::new();
    let results = check_data(
        &mut adapter,
        &VenueSymbol::new("aaa"),
        1_000,
        &DeclarationCheck::Diverges { detail: "tick 0.01 declared, 0.10 enforced".into() },
    )
    .await;

    let check = results.iter().find(|r| r.name == "declaration-matches-live-behaviour").unwrap();
    assert!(!check.is_pass());
    assert!(!is_releasable(&results));
}

/// Not asked and unreachable land together, and neither is a pass.
#[tokio::test]
async fn an_unverified_declaration_is_not_a_pass_and_holds_the_release() {
    let mut adapter = GoodAdapter::new();
    let results =
        check_data(&mut adapter, &VenueSymbol::new("aaa"), 1_000, &DeclarationCheck::Unverified)
            .await;

    let check = results.iter().find(|r| r.name == "declaration-matches-live-behaviour").unwrap();
    assert!(!check.is_pass(), "unverified is never a pass");
    assert!(!is_releasable(&results), "and it holds the release");
}

/// One adapter fault stays inside that adapter.
#[tokio::test]
async fn an_adapter_that_panics_does_not_take_the_caller_with_it() {
    let previous = std::panic::take_hook();
    // Silences the default panic hook for the panic this test causes on purpose.
    std::panic::set_hook(Box::new(|_| {}));

    let panicking: Answer<'_, u8> = Box::pin(async { panic!("the venue's own code gave up") });
    let contained = contain(panicking).await;

    std::panic::set_hook(previous);

    assert_eq!(contained, Err(Panicked), "the panic escaped the container");

    let fine: Answer<'_, u8> = Box::pin(async { 7 });
    assert_eq!(contain(fine).await, Ok(7));
}

// GoodAdapter accepts everything, so the wrapper is the only thing under test.

/// A never-sent order has a known fate. Queueing one would send an operator chasing a phantom.
#[test]
fn a_never_sent_order_is_not_queued_for_reconciliation() {
    assert!(!VenueOutcome::NotSent.needs_reconciliation(Attempted::Submit));
    assert!(!VenueOutcome::NotSent.needs_reconciliation(Attempted::Cancel));
}
