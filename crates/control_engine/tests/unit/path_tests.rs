// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The order path's gates: a deliberate breach drives the safe state and the reason is recorded.

use bijective_venue::profile::{SupportDepth, SupportEntry};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::loop_::ProposalWindow;
use std::sync::atomic::{AtomicU64, Ordering};

use bijective_common::clock::{Clock, Nanos, SequenceClock};
use bijective_event_store::store::read_range;
use bijective_limits::{
    Amount, Breach, Evaluation, ExposureSnapshot, Freshness, LimitKind, PositionHeld, Ratio,
    ReduceOnlySemantics, SafeState, SafeStateReason, Scope, ScopeLevel, ScopedBreach, Threshold,
};
use bijective_model::{AccountSnapshot, Clearance, OrderRequest, Side};
use bijective_risk::limit_state::Budget;
use bijective_venue::contract::{Answer, CapabilityDeclaration, ConnectError, VenueExecution};
use bijective_venue::normalise::{Rejection, Unknown, VenueOutcome};
use bijective_venue::profile::{
    DataClass, FundingModel, IdentifierRules, Instrument, InstrumentCatalogue, InstrumentId,
    InstrumentKind, InstrumentStatus, OrderTypeSupport, PrecisionRules, PriceValidity, ReduceOnly,
    Transport, TransportProfile, VenueProfile, VenueSymbol, declaration::PROFILE_FORMAT,
};

use crate::path::{Decision, OrderPath, Placement, Spend, SpendError};
use crate::wire::{
    ControlEvent, InstrumentRefusal, Participation, PostconditionOutcome, RefusalReason, decode,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn root(name: &str) -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join(format!("bijective-order-path-{name}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch root");
    dir
}

fn profile() -> VenueProfile {
    let mut transports = BTreeMap::new();
    transports.insert(
        Transport::Stream,
        TransportProfile {
            funding: FundingModel::Continuous,
            data_classes: vec![DataClass::TopOfBook],
            max_staleness_ns: Some(5_000_000_000),
        },
    );
    VenueProfile {
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
        identifier_rules: IdentifierRules {
            allowed: "abcdefghijklmnopqrstuvwxyz0123456789-".into(),
            max_len: 16,
        },
        transformations: vec![],
        support: SupportEntry::declare("venue-a", SupportDepth::Full, true).expect("declared"),
    }
}

fn declaration() -> CapabilityDeclaration {
    CapabilityDeclaration::from_profile(&profile())
}

/// A venue that answers as told, counts calls, and reads the control stream when asked. The read
/// proves intent-before-submit, which the finished stream cannot (see `docs/concepts/order_path.md`).
struct Venue {
    declaration: CapabilityDeclaration,
    answer: VenueOutcome,
    submissions: usize,
    root: PathBuf,
    /// What the control stream held when `submit` was entered.
    seen_at_submit: Vec<ControlEvent>,
}

impl Venue {
    fn answering(answer: VenueOutcome) -> Self {
        Self::at(root("venue-unused"), answer)
    }

    fn listing(status: InstrumentStatus, minimum_size: i128) -> Self {
        let mut v = Self::answering(VenueOutcome::Accepted);
        v.declaration = declaration_where(|i| {
            i.status = status;
            i.minimum_size = minimum_size;
        });
        v
    }

    fn at(root: PathBuf, answer: VenueOutcome) -> Self {
        Self {
            declaration: declaration(),
            answer,
            submissions: 0,
            root,
            seen_at_submit: Vec::new(),
        }
    }
}

impl VenueExecution for Venue {
    fn declaration(&self) -> &CapabilityDeclaration {
        &self.declaration
    }

    fn submit<'a>(
        &'a mut self,
        _clearance: &'a Clearance,
        _order: &'a OrderRequest,
        _now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        Box::pin(async move {
            self.submissions += 1;
            self.seen_at_submit = journal(&self.root);
            self.answer.clone()
        })
    }

    fn cancel<'a>(
        &'a mut self,
        _instrument: &'a VenueSymbol,
        _client_id: &'a str,
        _now_ns: i128,
    ) -> Answer<'a, VenueOutcome> {
        Box::pin(async move { self.answer.clone() })
    }

    fn account(&mut self, now_ns: i128) -> Answer<'_, Result<AccountSnapshot, ConnectError>> {
        Box::pin(async move {
            Ok(AccountSnapshot { observed_at_ns: now_ns, positions: Vec::new(), margin: None })
        })
    }
}

/// A declaration whose single instrument has been adjusted.
fn declaration_where(adjust: impl FnOnce(&mut Instrument)) -> CapabilityDeclaration {
    let mut p = profile();
    let mut instrument = p.instruments.get(&VenueSymbol::new("aaa")).unwrap().clone();
    adjust(&mut instrument);
    p.instruments = InstrumentCatalogue::from_instruments([instrument]);
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

fn clear() -> Evaluation {
    Evaluation { breaches: Vec::new(), unjudged: Vec::new(), freshness: Freshness::Fresh }
}

/// A breach with the subject its level implies. An evaluation carries both.
fn breach(level: ScopeLevel, kind: LimitKind) -> ScopedBreach {
    let scope = match level {
        ScopeLevel::Portfolio => Scope::Portfolio,
        ScopeLevel::Desk => Scope::Desk("alpha".into()),
        ScopeLevel::Venue => Scope::Venue("venue-a".into()),
        ScopeLevel::Strategy => Scope::Strategy("carry".into()),
        ScopeLevel::Underlying => Scope::Underlying("btc".into()),
        ScopeLevel::Instrument => Scope::Instrument("btc-perp".into()),
    };
    ScopedBreach {
        scope,
        breach: Breach {
            level,
            kind,
            observed: Threshold::Money(Amount::from_scaled(1_500)),
            maximum: Threshold::Money(Amount::from_scaled(1_000)),
        },
    }
}

/// A commitment against the portfolio and one desk. Draws nothing where no limit is declared.
fn spend() -> Spend {
    Spend::declare(Amount::from_scaled(1_000), vec![Scope::Portfolio, Scope::Desk("alpha".into())])
        .expect("a positive amount and at least one scope")
}

/// Put a money limit in force for `scope`, the only way one goes into force.
fn declare(path: &mut OrderPath, scope: Scope, maximum: i64) {
    let (state, writer) = path.limits_mut();
    state
        .declare(
            writer,
            scope,
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(i128::from(maximum))),
            1,
        )
        .expect("declare");
}

fn drawn(path: &mut OrderPath, scope: &Scope) -> Budget {
    let (state, _) = path.limits_mut();
    state.budget(scope, LimitKind::GrossExposure)
}

fn journal(root: &std::path::Path) -> Vec<ControlEvent> {
    read_range(root, "control", 0, u64::MAX)
        .unwrap_or_default()
        .into_iter()
        .map(|entry| decode(&entry.payload).expect("decode"))
        .collect()
}

fn clock() -> SequenceClock {
    SequenceClock::starting_at(1_000)
}

/// An intent written before the venue was recorded still reads, with the venue absent rather than
/// invented.
#[test]
fn an_intent_from_before_the_venue_was_recorded_reads_with_no_venue() {
    let current = ControlEvent::Intent {
        decision_id: "dec-old".into(),
        venue: None,
        owner: None,
        from_position: 3,
        request: order(),
        at_ns: 9,
    };
    let mut bytes = crate::wire::encode(&current).expect("encode");
    // The expiry and stop flags and the two terms bytes, written after the owner and the venue.
    bytes.truncate(bytes.len() - 4);
    bytes[2..4].copy_from_slice(&3u16.to_be_bytes());
    assert_eq!(crate::wire::decode(&bytes).expect("a version 3 intent"), current);
}

/// An intent's expiry round-trips. One recorded before layout 13 does not expire (decision 0014).
#[test]
fn an_intents_expiry_round_trips_and_an_older_intent_does_not_expire() {
    let mut request = order();
    request.terms.expire_at_ns = Some(1_800_000_000_000_000_000);
    let current = ControlEvent::Intent {
        decision_id: "dec-gtd".into(),
        venue: Some("venue-a".into()),
        owner: None,
        from_position: 3,
        request,
        at_ns: 9,
    };
    let bytes = crate::wire::encode(&current).expect("encode");
    assert_eq!(crate::wire::decode(&bytes).expect("decode"), current);
    let mut older = bytes.clone();
    older.truncate(older.len() - 17);
    older[2..4].copy_from_slice(&12u16.to_be_bytes());
    let ControlEvent::Intent { request, .. } = crate::wire::decode(&older).expect("version 12")
    else {
        panic!("not an intent")
    };
    assert_eq!(request.terms.expire_at_ns, None);
}

/// A summary's ratios round-trip. One recorded before layout 14 reads them as unmeasured.
#[test]
fn a_summarys_ratios_round_trip_and_an_older_summary_has_none() {
    let figures = crate::wire::Valued {
        gross_scaled: Some(5_000),
        net_scaled: Some(5_000),
        daily_loss_scaled: None,
        observed_at_ns: 4,
        leverage_scaled: Some(25_000),
        margin_use_scaled: Some(3_500),
        equity_scaled: Some(-7),
    };
    let current = ControlEvent::PortfolioValued { figures, at_ns: 5 };
    let bytes = crate::wire::encode(&current).expect("encode");
    assert_eq!(crate::wire::decode(&bytes).expect("decode"), current);
    let mut fifteen = bytes.clone();
    fifteen.truncate(fifteen.len() - 17);
    fifteen[2..4].copy_from_slice(&15u16.to_be_bytes());
    let ControlEvent::PortfolioValued { figures: read, .. } =
        crate::wire::decode(&fifteen).expect("version 15")
    else {
        panic!("not a summary")
    };
    assert_eq!(read, crate::wire::Valued { equity_scaled: None, ..figures });
    let mut older = bytes.clone();
    older.truncate(older.len() - 51);
    older[2..4].copy_from_slice(&13u16.to_be_bytes());
    let ControlEvent::PortfolioValued { figures, .. } =
        crate::wire::decode(&older).expect("version 13")
    else {
        panic!("not a summary")
    };
    assert_eq!((figures.leverage_scaled, figures.margin_use_scaled), (None, None));
    assert_eq!(figures.gross_scaled, Some(5_000));
}

/// An adjustment's owner round-trips. One recorded before layout 6 reads as nobody's.
#[test]
fn an_adjustment_reads_with_its_owner_and_an_older_one_with_none() {
    let adjusted = |owner| ControlEvent::PositionAdjusted {
        adjustment: crate::wire::Adjustment {
            venue: "venue-a".into(),
            adjustment_id: "adj-1".into(),
            actor: "risk".into(),
            instrument: "aaa".into(),
            side: Side::Buy,
            quantity_scaled: 4,
            price_scaled: 100,
            reason: "moved in".into(),
            owner,
        },
        at_ns: 3,
    };
    let owned = adjusted(Some(bijective_model::Owner {
        desk: "alpha".into(),
        strategy: Some("trend".into()),
    }));
    assert_eq!(
        crate::wire::decode(&crate::wire::encode(&owned).expect("encode")).expect("decode"),
        owned
    );

    let mut bytes = crate::wire::encode(&adjusted(None)).expect("encode");
    bytes.pop();
    bytes[2..4].copy_from_slice(&5u16.to_be_bytes());
    assert_eq!(crate::wire::decode(&bytes).expect("a layout 5 adjustment"), adjusted(None));
}

/// An intent written before intents named their owner still reads, with no owner rather than an
/// invented one: its fills then count against every desk and strategy.
#[test]
fn an_intent_from_before_the_owner_was_recorded_reads_with_no_owner() {
    let current = ControlEvent::Intent {
        decision_id: "dec-old".into(),
        venue: Some("venue-a".into()),
        owner: None,
        from_position: 3,
        request: order(),
        at_ns: 9,
    };
    let mut bytes = crate::wire::encode(&current).expect("encode");
    // The owner marker, the two bytes of terms version 10 and the stop and expiry flags.
    bytes.truncate(bytes.len() - 4);
    bytes[2..4].copy_from_slice(&4u16.to_be_bytes());
    assert_eq!(crate::wire::decode(&bytes).expect("a version 4 intent"), current);
}

/// Terms round-trip with the intent. An intent recorded before them reads as the default terms,
/// what every order was placed under then (decision 0009).
#[test]
fn an_intents_terms_round_trip_and_an_older_intent_reads_the_default() {
    let mut request = order();
    request.terms = bijective_model::OrderTerms {
        time_in_force: bijective_model::TimeInForce::FillOrKill,
        post_only: false,
        stop_price_scaled: Some(2_650_000),
        expire_at_ns: None,
    };
    let current = ControlEvent::Intent {
        decision_id: "dec-terms".into(),
        venue: Some("venue-a".into()),
        owner: None,
        from_position: 3,
        request,
        at_ns: 9,
    };
    let bytes = crate::wire::encode(&current).expect("encode");
    assert_eq!(crate::wire::decode(&bytes).expect("decode"), current);

    let mut older = bytes.clone();
    // The expiry flag, the stop (a flag and sixteen bytes), then the two terms bytes.
    older.truncate(older.len() - 20);
    older[2..4].copy_from_slice(&9u16.to_be_bytes());
    let ControlEvent::Intent { request, .. } = crate::wire::decode(&older).expect("version 9")
    else {
        panic!("not an intent")
    };
    assert_eq!(request.terms, bijective_model::OrderTerms::default());
}

/// The owner recorded with an intent is the desk and strategy the spend binds: the record of who
/// placed an order and the budget it drew on cannot disagree.
#[test]
fn a_spend_names_the_desk_and_strategy_it_binds_as_its_owner() {
    let bound = Spend::declare(
        Amount::from_scaled(1),
        vec![
            Scope::Portfolio,
            Scope::Desk("alpha".into()),
            Scope::Venue("venue-a".into()),
            Scope::Strategy("trend".into()),
        ],
    )
    .expect("spend");
    assert_eq!(
        bound.owner(),
        Some(bijective_model::Owner { desk: "alpha".into(), strategy: Some("trend".into()) })
    );

    let deskless = Spend::declare(Amount::from_scaled(1), vec![Scope::Portfolio]).expect("spend");
    assert_eq!(deskless.owner(), None, "no desk is no owner, not an owner with an empty desk");
}

/// Every class round-trips, and encoding is stable. 's obligation, per class.
#[test]
fn every_class_round_trips_and_encodes_the_same_way_twice() {
    let events = [
        ControlEvent::Held {
            proposal: Proposal {
                reduce_only: true,
                parent: Some("entry".into()),
                oco: Some("bracket".into()),
                ..signal("tp", 10)
            },
            from_position: 7,
            at_ns: 8,
        },
        ControlEvent::InstrumentScales {
            venue: "venue-a".into(),
            instrument: VenueSymbol::new("aaa"),
            price_scale: 4,
            quantity_scale: 9,
            at_ns: 4,
            identity: Some("venue-a.AAA".into()),
            quote_currency: None,
        },
        ControlEvent::DayOpened { day: 20_000, unrealized_scaled: -123_456_789, at_ns: 5 },
        ControlEvent::DayPeak { day: 20_000, result_scaled: 42_000_000, at_ns: 6 },
        ControlEvent::ScopesValued {
            figures: vec![
                crate::wire::ScopeFigure {
                    scope: Scope::Underlying("btc".into()),
                    kind: LimitKind::NetExposure,
                    value_scaled: -7_000,
                },
                crate::wire::ScopeFigure {
                    scope: Scope::Desk("alpha".into()),
                    kind: LimitKind::GrossExposure,
                    value_scaled: 9_000,
                },
            ],
            at_ns: 6,
        },
        ControlEvent::PortfolioValued {
            figures: crate::wire::Valued {
                gross_scaled: Some(5_000),
                net_scaled: Some(-1_200),
                daily_loss_scaled: None,
                observed_at_ns: 4,
                leverage_scaled: None,
                margin_use_scaled: None,
                equity_scaled: Some(250_000),
            },
            at_ns: 5,
        },
        ControlEvent::Intent {
            decision_id: "dec-1".into(),
            venue: Some("venue-a".into()),
            owner: Some(bijective_model::Owner {
                desk: "alpha".into(),
                strategy: Some("trend".into()),
            }),
            from_position: 7,
            request: order(),
            at_ns: 7,
        },
        ControlEvent::Intent {
            decision_id: "dec-market".into(),
            venue: Some("venue-a".into()),
            owner: Some(bijective_model::Owner { desk: "alpha".into(), strategy: None }),
            from_position: 7,
            request: OrderRequest {
                price_scaled: None,
                reduce_only: true,
                side: Side::Sell,
                ..order()
            },
            at_ns: 8,
        },
        ControlEvent::Settled {
            client_id: "abc123".into(),
            outcome: VenueOutcome::Accepted,
            at_ns: 9,
        },
        ControlEvent::Settled {
            client_id: "abc123".into(),
            outcome: VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: Some(500) }),
            at_ns: 10,
        },
        ControlEvent::Settled {
            client_id: "abc123".into(),
            outcome: VenueOutcome::Rejected(Rejection::RateLimited { retry_after_ns: None }),
            at_ns: 11,
        },
        ControlEvent::Settled {
            client_id: "abc123".into(),
            outcome: VenueOutcome::Rejected(Rejection::Unclassified {
                venue_code: "110094".into(),
            }),
            at_ns: 12,
        },
        ControlEvent::Settled {
            client_id: "abc123".into(),
            outcome: VenueOutcome::Unknown(Unknown::Timeout),
            at_ns: 13,
        },
        ControlEvent::Refused {
            decision_id: "dec-2".into(),
            from_position: 3,
            reason: RefusalReason::NotActive { state: SafeState::Halted },
            at_ns: 14,
        },
        ControlEvent::Refused {
            decision_id: "dec-3".into(),
            from_position: 3,
            reason: RefusalReason::Unjudged { kind: LimitKind::Drawdown },
            at_ns: 15,
        },
        ControlEvent::Refused {
            decision_id: "dec-4".into(),
            from_position: 3,
            reason: RefusalReason::Stale { age_ns: 42 },
            at_ns: 16,
        },
        ControlEvent::Refused {
            decision_id: "dec-5".into(),
            from_position: 3,
            reason: RefusalReason::ClockSkew { skew_ns: -9 },
            at_ns: 17,
        },
        ControlEvent::Refused {
            decision_id: "dec-6".into(),
            from_position: 3,
            reason: RefusalReason::LimitsUnrecovered,
            at_ns: 18,
        },
        ControlEvent::Refused {
            decision_id: "dec-7".into(),
            from_position: 3,
            reason: RefusalReason::NoBudget {
                scope: Scope::Desk("alpha".into()),
                kind: LimitKind::GrossExposure,
            },
            at_ns: 19,
        },
        ControlEvent::Refused {
            decision_id: "dec-9".into(),
            from_position: 3,
            reason: RefusalReason::InstrumentRefused {
                symbol: VenueSymbol::new("aaa"),
                cause: InstrumentRefusal::Delisted,
            },
            at_ns: 21,
        },
        ControlEvent::Refused {
            decision_id: "dec-8".into(),
            from_position: 3,
            reason: RefusalReason::NoBudget {
                scope: Scope::Portfolio,
                kind: LimitKind::NetExposure,
            },
            at_ns: 20,
        },
        ControlEvent::SafeStateDriven {
            target: SafeState::Halted,
            reason: SafeStateReason::Breached(
                breach(ScopeLevel::Portfolio, LimitKind::GrossExposure).breach,
            ),
            arrived: true,
            at_ns: 19,
            scope: None,
        },
        ControlEvent::SafeStateDriven {
            target: SafeState::Reducing,
            reason: SafeStateReason::Unjudgeable { kind: LimitKind::Concentration },
            arrived: false,
            at_ns: 20,
            scope: None,
        },
        ControlEvent::SafeStateDriven {
            target: SafeState::Halted,
            reason: SafeStateReason::InputsStale { stale_for_ns: 3_000_000_000 },
            arrived: true,
            at_ns: 21,
            scope: None,
        },
        ControlEvent::SafeStateDriven {
            target: SafeState::Halted,
            reason: SafeStateReason::NeverPrimed,
            arrived: true,
            at_ns: 22,
            scope: None,
        },
        ControlEvent::SafeStatePostcondition {
            scope: Scope::Venue("venue-a".into()),
            kind: LimitKind::GrossExposure,
            before: Threshold::Money(Amount::from_scaled(900)),
            outcome: PostconditionOutcome::Held {
                after: Threshold::Money(Amount::from_scaled(880)),
            },
            at_ns: 23,
        },
        ControlEvent::SafeStatePostcondition {
            scope: Scope::Portfolio,
            kind: LimitKind::Drawdown,
            before: Threshold::Ratio(Ratio::from_scaled(1_200)),
            outcome: PostconditionOutcome::Increased {
                after: Threshold::Ratio(Ratio::from_scaled(1_500)),
            },
            at_ns: 24,
        },
        ControlEvent::SafeStatePostcondition {
            scope: Scope::Venue("venue-b".into()),
            kind: LimitKind::NetExposure,
            before: Threshold::Money(Amount::from_scaled(400)),
            outcome: PostconditionOutcome::Unverifiable,
            at_ns: 25,
        },
    ];

    for event in &events {
        let first = crate::wire::encode(event).expect("encode");
        let second = crate::wire::encode(event).expect("encode");
        assert_eq!(first, second, "encoding must not vary between calls");
        assert_eq!(&decode(&first).expect("decode"), event);
    }
}

/// A ratio threshold keeps its unit through the round trip: a proportion read as money is invisible.
#[test]
fn a_ratio_breach_keeps_its_unit() {
    let event = ControlEvent::SafeStateDriven {
        target: SafeState::Halted,
        reason: SafeStateReason::Breached(Breach {
            level: ScopeLevel::Desk,
            kind: LimitKind::Leverage,
            observed: Threshold::Ratio(Ratio::from_scaled(30_000)),
            maximum: Threshold::Ratio(Ratio::from_scaled(20_000)),
        }),
        arrived: true,
        at_ns: 1,
        scope: None,
    };

    let bytes = crate::wire::encode(&event).expect("encode");
    assert_eq!(decode(&bytes).expect("decode"), event);
}

/// A payload from another domain is refused, not read as ours.
#[test]
fn a_foreign_class_is_refused() {
    let payload =
        bijective_risk::limit_state::encode(&bijective_risk::limit_state::LimitEvent::Released {
            scope: bijective_limits::Scope::Portfolio,
            kind: LimitKind::GrossExposure,
            amount: Amount::from_scaled(1),
            reason: bijective_risk::limit_state::ReleaseReason::PositionClosed,
            decision_id: None,
        })
        .expect("encode");

    assert!(decode(&payload).is_err());
}

/// A fresh node is halted, so the first order it is handed does not go anywhere.
#[tokio::test]
async fn a_fresh_node_refuses_and_never_reaches_the_venue() {
    let r = root("fresh");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");

    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-1",
                venue: "venue-a",
                from_position: 0,
                request: &order(),
                spend: &spend(),
                evaluation: &clear(),
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");

    assert_eq!(
        outcome,
        Placement::Refused { reason: RefusalReason::NotActive { state: SafeState::Halted } }
    );
    assert_eq!(venue.submissions, 0, "a refusal must not reach the venue");
    drop(path);
    assert!(matches!(journal(&r).as_slice(), [ControlEvent::Refused { .. }]));
}

/// A limit history that cannot be replayed refuses to open the path at all.
#[test]
fn a_limit_history_that_will_not_replay_refuses_to_open() {
    let r = root("corrupt-limits");
    {
        let mut path = OrderPath::open(&r, "control", "limits").expect("open");
        declare(&mut path, Scope::Portfolio, 1_000);
    }

    let seg = r.join("limits").join("000000000000.seg");
    let mut bytes = std::fs::read(&seg).expect("read segment");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&seg, &bytes).expect("write segment");

    assert!(OrderPath::open(&r, "control", "limits").is_err());
}

/// A deliberate breach drives the safe state and the reason is recorded.
#[tokio::test]
async fn a_deliberate_breach_drives_the_safe_state_and_records_why() {
    let r = root("breach");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let breached = Evaluation {
        breaches: vec![breach(ScopeLevel::Portfolio, LimitKind::GrossExposure)],
        ..clear()
    };

    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-1",
                venue: "venue-a",
                from_position: 0,
                request: &order(),
                spend: &spend(),
                evaluation: &breached,
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");

    assert_eq!(outcome, Placement::Refused { reason: RefusalReason::Breached });
    assert_eq!(venue.submissions, 0);
    assert_eq!(path.trading_state(), SafeState::Halted);

    drop(path);
    let written = journal(&r);
    match written.as_slice() {
        [
            ControlEvent::SafeStateDriven { target, reason, arrived, .. },
            ControlEvent::Refused { .. },
        ] => {
            assert_eq!(*target, SafeState::Halted);
            assert!(*arrived, "the read-back must confirm the platform stopped");
            assert_eq!(
                *reason,
                SafeStateReason::Breached(
                    breach(ScopeLevel::Portfolio, LimitKind::GrossExposure).breach
                ),
                "the reason carries the figure and the threshold, both with their units"
            );
        }
        other => panic!("expected a drive then a refusal, got {other:?}"),
    }
}

/// An instrument breach at a venue that refuses reduce-only at placement is served by Reducing.
#[tokio::test]
async fn an_instrument_breach_on_a_held_position_reduces_rather_than_halts() {
    let r = root("reducing");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let breached = Evaluation {
        breaches: vec![breach(ScopeLevel::Instrument, LimitKind::Concentration)],
        ..clear()
    };

    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &breached,
            position_held: PositionHeld::Yes,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works");

    assert_eq!(path.trading_state(), SafeState::Reducing);
}

/// A venue that clamps reduce-only at fill time gets a halt instead.
#[tokio::test]
async fn a_fill_time_clamp_escalates_to_a_halt() {
    let r = root("clamp");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let breached = Evaluation {
        breaches: vec![breach(ScopeLevel::Instrument, LimitKind::Concentration)],
        ..clear()
    };

    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &breached,
            position_held: PositionHeld::Yes,
            semantics: ReduceOnlySemantics::FillTimeClamp,
        },
    )
    .await
    .expect("the path itself works");

    assert_eq!(path.trading_state(), SafeState::Halted);
}

/// Several breaches each get their own record, and the platform ends where the heaviest put it.
#[tokio::test]
async fn every_breach_is_recorded_and_the_heaviest_decides_the_state() {
    let r = root("several");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let breached = Evaluation {
        breaches: vec![
            breach(ScopeLevel::Instrument, LimitKind::Concentration),
            breach(ScopeLevel::Portfolio, LimitKind::GrossExposure),
        ],
        ..clear()
    };

    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &breached,
            position_held: PositionHeld::Yes,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works");

    assert_eq!(path.trading_state(), SafeState::Halted);
    drop(path);
    let drives = journal(&r)
        .into_iter()
        .filter(|e| matches!(e, ControlEvent::SafeStateDriven { .. }))
        .count();
    assert_eq!(drives, 2, "each breach explains itself");
}

/// A relaxing escalation is not a failure to apply: arrived:false would send an operator hunting a fault that is not there.
#[tokio::test]
async fn a_lighter_state_than_the_one_in_force_is_not_a_failed_drive() {
    let r = root("no-relax");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");

    // Halted is where a fresh node starts, so this is the ordinary case.
    let breached = Evaluation {
        breaches: vec![breach(ScopeLevel::Instrument, LimitKind::Concentration)],
        ..clear()
    };
    path.resume(SafeState::Active);
    path.resume(SafeState::Halted);

    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &breached,
            position_held: PositionHeld::Yes,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works");

    assert_eq!(path.trading_state(), SafeState::Halted, "never relaxed");
    drop(path);
    // Halted before the breach, so the order is refused on the state and no drive is recorded.
    assert!(matches!(
        journal(&r).as_slice(),
        [ControlEvent::Refused { reason: RefusalReason::NotActive { .. }, .. }]
    ));
}

/// An unjudged limit halts, and names the limit.
#[tokio::test]
async fn an_unjudged_limit_halts_and_names_itself() {
    let r = root("unjudged");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let unjudged = Evaluation { unjudged: vec![LimitKind::Drawdown], ..clear() };

    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-1",
                venue: "venue-a",
                from_position: 0,
                request: &order(),
                spend: &spend(),
                evaluation: &unjudged,
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");

    assert_eq!(
        outcome,
        Placement::Refused { reason: RefusalReason::Unjudged { kind: LimitKind::Drawdown } }
    );
    assert_eq!(path.trading_state(), SafeState::Halted);
    drop(path);
    assert!(journal(&r).iter().any(|e| matches!(
        e,
        ControlEvent::SafeStateDriven {
            reason: SafeStateReason::Unjudgeable { kind: LimitKind::Drawdown },
            ..
        }
    )));
}

/// Stale figures refuse the order and drive nothing. The pre-trade check owns the staleness bound.
#[tokio::test]
async fn stale_figures_refuse_without_driving_a_safe_state() {
    let r = root("stale");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let stale = Evaluation { freshness: Freshness::Stale { age_ns: 9_000 }, ..clear() };

    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-1",
                venue: "venue-a",
                from_position: 0,
                request: &order(),
                spend: &spend(),
                evaluation: &stale,
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");

    assert_eq!(outcome, Placement::Refused { reason: RefusalReason::Stale { age_ns: 9_000 } });
    assert_eq!(venue.submissions, 0);
    assert_eq!(path.trading_state(), SafeState::Active, "the order stops; the platform does not");
    drop(path);
    assert!(!journal(&r).iter().any(|e| matches!(e, ControlEvent::SafeStateDriven { .. })));
}

/// Figures stamped ahead of the instant judged are refused as skew, not as staleness.
#[tokio::test]
async fn figures_from_the_future_are_refused_as_skew() {
    let r = root("skew");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let skewed = Evaluation { freshness: Freshness::Implausible { skew_ns: 4_000 }, ..clear() };

    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-1",
                venue: "venue-a",
                from_position: 0,
                request: &order(),
                spend: &spend(),
                evaluation: &skewed,
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");

    assert_eq!(outcome, Placement::Refused { reason: RefusalReason::ClockSkew { skew_ns: 4_000 } });
}

/// A clear evaluation reaches the venue, and the intent is on disk before the settlement.
#[tokio::test]
async fn a_cleared_order_is_written_before_it_is_sent() {
    let r = root("sent");
    let mut venue = Venue::at(r.clone(), VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-1",
                venue: "venue-a",
                from_position: 0,
                request: &order(),
                spend: &spend(),
                evaluation: &clear(),
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");

    assert_eq!(outcome, Placement::Sent { outcome: VenueOutcome::Accepted });
    assert_eq!(venue.submissions, 1);
    // The intent was already durable when the venue was asked.
    assert!(
        matches!(
            venue.seen_at_submit.as_slice(),
            [ControlEvent::InstrumentScales { .. }, ControlEvent::Intent { .. }]
        ),
        "the intent must be on disk before the venue is called, not merely before the settlement; \
         saw {:?}",
        venue.seen_at_submit
    );

    drop(path);
    match journal(&r).as_slice() {
        [
            ControlEvent::InstrumentScales {
                venue: scaled_on,
                price_scale,
                quantity_scale,
                identity,
                quote_currency,
                ..
            },
            ControlEvent::Intent { decision_id, venue, owner, request, .. },
            ControlEvent::Settled { client_id, outcome, .. },
        ] => {
            // The record says what the intent's integers stand for, before the intent.
            assert_eq!(scaled_on, "venue-a");
            assert!(*price_scale > 0 || *quantity_scale > 0, "the profile's scales");
            assert!(
                identity.is_some() && quote_currency.is_some(),
                "the record names the instrument"
            );
            assert_eq!(decision_id, "dec-1");
            assert_eq!(venue.as_deref(), Some("venue-a"), "a restart must know where it was sent");
            assert_eq!(
                owner.as_ref().map(|o| o.desk.as_str()),
                Some("alpha"),
                "the desk an order was placed for is recorded with it"
            );
            assert_eq!(request, &order());
            assert_eq!(client_id, "abc123");
            assert_eq!(outcome, &VenueOutcome::Accepted);
        }
        other => panic!("expected the scales, an intent then a settlement, got {other:?}"),
    }
}

/// An answer the venue never gave is recorded as unknown, never as a refusal.
#[tokio::test]
async fn an_unknown_outcome_stays_unknown_on_disk() {
    let r = root("unknown");
    let mut venue = Venue::at(r.clone(), VenueOutcome::Unknown(Unknown::ConnectionLost));
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works");

    drop(path);
    assert!(journal(&r).iter().any(|e| matches!(
        e,
        ControlEvent::Settled { outcome: VenueOutcome::Unknown(Unknown::ConnectionLost), .. }
    )));
}

/// The clock the path stamps with is the interface, not the operating system's.
#[test]
fn the_path_takes_its_time_from_the_clock_it_was_given() {
    let c = SequenceClock::starting_at(4_242);
    assert_eq!(c.now_ns(), 4_242 as Nanos);
}

/// A spend of nothing, or a spend bound to nothing, is not a commitment.
#[test]
fn a_spend_must_be_positive_and_bound_to_something() {
    assert_eq!(
        Spend::declare(Amount::from_scaled(0), vec![Scope::Portfolio]),
        Err(SpendError::NotPositive)
    );
    assert_eq!(
        Spend::declare(Amount::from_scaled(-5), vec![Scope::Portfolio]),
        Err(SpendError::NotPositive)
    );
    assert_eq!(Spend::declare(Amount::from_scaled(5), vec![]), Err(SpendError::NoScopes));
}

/// Every scope that declares a money limit is drawn, and by the same amount.
#[tokio::test]
async fn an_order_draws_from_every_scope_that_declares_a_limit() {
    let r = root("draws");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 5_000);
    declare(&mut path, Scope::Desk("alpha".into()), 3_000);
    path.resume(SafeState::Active);

    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works");

    assert_eq!(drawn(&mut path, &Scope::Portfolio), Budget::Remaining(Amount::from_scaled(4_000)));
    assert_eq!(
        drawn(&mut path, &Scope::Desk("alpha".into())),
        Budget::Remaining(Amount::from_scaled(2_000))
    );
}

/// One scope short refuses the order and nothing is drawn anywhere.
#[tokio::test]
async fn a_scope_that_cannot_cover_the_spend_draws_nothing_anywhere() {
    let r = root("all-or-nothing");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 5_000);
    declare(&mut path, Scope::Desk("alpha".into()), 500);
    path.resume(SafeState::Active);

    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-1",
                venue: "venue-a",
                from_position: 0,
                request: &order(),
                spend: &spend(),
                evaluation: &clear(),
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");

    assert_eq!(
        outcome,
        Placement::Refused {
            reason: RefusalReason::NoBudget {
                scope: Scope::Desk("alpha".into()),
                kind: LimitKind::GrossExposure
            }
        }
    );
    assert_eq!(venue.submissions, 0);
    assert_eq!(
        drawn(&mut path, &Scope::Portfolio),
        Budget::Remaining(Amount::from_scaled(5_000)),
        "the scope that could cover it must not have been drawn"
    );
}

/// A short of `quantity` in `aaa` on venue-a, accepted by a risk officer.
fn hold_short(path: &mut OrderPath, quantity: i128) {
    let command = bijective_serialization::signal::Command {
        id: format!("adj-{quantity}"),
        actor: "risk-desk".into(),
        role: bijective_authority::Role::RiskOfficer,
        instruction: Instruction::AdjustPosition {
            venue: "venue-a".into(),
            adjustment: bijective_serialization::signal::PositionAdjustment {
                instrument: "aaa".into(),
                side: Side::Sell,
                quantity_scaled: quantity,
                price_scaled: 2_654_375,
                reason: "held for the test".into(),
                owner: None,
            },
        },
        at_ns: 0,
    };
    path.carry_out(&command, None, 0, 0).expect("adjusted");
}

/// A reduce-only order draws nothing: it only lowers exposure.
#[tokio::test]
async fn a_reduce_only_order_draws_nothing() {
    let r = root("reduce-only");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_200);
    hold_short(&mut path, 20);
    path.resume(SafeState::Active);

    // Twice the declared allowance, and both go, because neither reserves anything.
    for id in ["dec-1", "dec-2"] {
        let outcome = path
            .place(
                &mut venue,
                &clock(),
                &Decision {
                    id,
                    venue: "venue-a",
                    from_position: 0,
                    request: &OrderRequest { reduce_only: true, ..order() },
                    spend: &spend(),
                    evaluation: &clear(),
                    position_held: PositionHeld::Yes,
                    semantics: ReduceOnlySemantics::PlacementPrecondition,
                },
            )
            .await
            .expect("the path itself works");
        assert!(matches!(outcome, Placement::Sent { .. }));
    }

    assert_eq!(drawn(&mut path, &Scope::Portfolio), Budget::Remaining(Amount::from_scaled(1_200)));
}

/// A venue rejection returns the reservation.
#[tokio::test]
async fn a_rejected_order_returns_its_reservation() {
    let r = root("rejected");
    let mut venue = Venue::answering(VenueOutcome::Rejected(Rejection::BelowMinimumSize));
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 5_000);
    path.resume(SafeState::Active);

    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works");

    assert_eq!(
        drawn(&mut path, &Scope::Portfolio),
        Budget::Remaining(Amount::from_scaled(5_000)),
        "the venue said it did not happen"
    );
}

/// An unknown answer keeps the reservation: the order may have filled, and reconciliation settles it.
#[tokio::test]
async fn an_unknown_answer_keeps_the_reservation() {
    let r = root("unknown-keeps");
    let mut venue = Venue::answering(VenueOutcome::Unknown(Unknown::Timeout));
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 5_000);
    path.resume(SafeState::Active);

    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works");

    assert_eq!(drawn(&mut path, &Scope::Portfolio), Budget::Remaining(Amount::from_scaled(4_000)));
}

/// A duplicate says the venue already holds this identifier: the order exists, so its
/// reservation stays drawn. Releasing it would under-count exposure by the whole order.
#[tokio::test]
async fn a_duplicate_answer_keeps_the_reservation() {
    let r = root("duplicate-keeps");
    let mut venue = Venue::answering(VenueOutcome::Rejected(Rejection::Duplicate));
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 5_000);
    path.resume(SafeState::Active);

    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works");

    assert_eq!(drawn(&mut path, &Scope::Portfolio), Budget::Remaining(Amount::from_scaled(4_000)));
}

/// A scope with no limit declared is skipped, not treated as an allowance of zero.
#[tokio::test]
async fn an_undeclared_scope_is_skipped_rather_than_refused() {
    let r = root("undeclared");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-1",
                venue: "venue-a",
                from_position: 0,
                request: &order(),
                spend: &spend(),
                evaluation: &clear(),
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");

    assert!(matches!(outcome, Placement::Sent { .. }));
    assert_eq!(drawn(&mut path, &Scope::Portfolio), Budget::Undeclared);
}

/// An instrument the venue's catalogue does not carry never reaches the venue.
#[tokio::test]
async fn an_unknown_instrument_is_refused_before_the_venue() {
    let r = root("unknown-instrument");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-1",
                venue: "venue-a",
                from_position: 0,
                request: &OrderRequest { instrument: VenueSymbol::new("zzz"), ..order() },
                spend: &spend(),
                evaluation: &clear(),
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");

    assert_eq!(
        outcome,
        Placement::Refused {
            reason: RefusalReason::InstrumentRefused {
                symbol: VenueSymbol::new("zzz"),
                cause: InstrumentRefusal::Unknown
            }
        }
    );
    assert_eq!(venue.submissions, 0);
}

/// A delisted instrument is refused here rather than by the venue.
#[tokio::test]
async fn a_delisted_instrument_is_refused() {
    let r = root("delisted");
    let mut venue = Venue::listing(InstrumentStatus::Delisted, 1);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-1",
                venue: "venue-a",
                from_position: 0,
                request: &order(),
                spend: &spend(),
                evaluation: &clear(),
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");

    assert_eq!(
        outcome,
        Placement::Refused {
            reason: RefusalReason::InstrumentRefused {
                symbol: VenueSymbol::new("aaa"),
                cause: InstrumentRefusal::Delisted
            }
        }
    );
    assert_eq!(venue.submissions, 0);
}

/// An order below the venue's minimum size does not make the round trip.
#[tokio::test]
async fn an_order_below_the_minimum_size_is_refused() {
    let r = root("below-minimum");
    let mut venue = Venue::listing(InstrumentStatus::Trading, 1_000);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    // `order()` carries a quantity of 10, well under this venue's minimum.
    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-1",
                venue: "venue-a",
                from_position: 0,
                request: &order(),
                spend: &spend(),
                evaluation: &clear(),
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");

    assert_eq!(
        outcome,
        Placement::Refused {
            reason: RefusalReason::InstrumentRefused {
                symbol: VenueSymbol::new("aaa"),
                cause: InstrumentRefusal::BelowMinimumSize
            }
        }
    );
    assert_eq!(venue.submissions, 0);
}

/// A halted instrument refuses without driving a safe state: a listing pause is not a breach.
#[tokio::test]
async fn a_halted_instrument_does_not_halt_the_platform() {
    let r = root("halted-instrument");
    let mut venue = Venue::listing(InstrumentStatus::Halted, 1);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works");

    assert_eq!(path.trading_state(), SafeState::Active);
    drop(path);
    assert!(!journal(&r).iter().any(|e| matches!(e, ControlEvent::SafeStateDriven { .. })));
}

/// A proposal becomes an order, draws its own spend, and reaches the venue. Asserts the parts
/// compose. The program loop is covered elsewhere.
#[tokio::test]
async fn an_admitted_proposal_draws_its_own_spend_and_reaches_the_venue() {
    use std::collections::BTreeSet;

    use bijective_risk::desks::admit;

    let r = root("bridge");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_000_000_000);
    path.resume(SafeState::Active);

    let mut instruments = BTreeSet::new();
    instruments.insert(InstrumentId::new("aaa"));
    let mut mandates = MandateBook::new();
    mandates.declare(
        "alpha",
        Mandate {
            instruments,
            venue_preference: vec!["venue-a".into()],
            maximum_per_order: Amount::from_scaled(1_000_000_000),
            price_collar_bps: 500,
        },
    );

    let mut reachable = BTreeMap::new();
    reachable.insert("venue-a".to_string(), declaration());

    let admitted = admit(
        &mandates,
        &reachable,
        &Proposal {
            desk: "alpha".into(),
            instrument: InstrumentId::new("aaa"),
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
        },
        "abc123",
    )
    .expect("the desk mandates this instrument on this venue");

    // 1.5 units at 2.00, computed at the instrument's precision rather than declared by the desk.
    assert_eq!(admitted.spend, Amount::from_scaled(300_000_000));

    let spend = Spend::declare(admitted.spend, admitted.scopes).expect("a positive, scoped spend");
    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-bridge",
                venue: "venue-a",
                from_position: 0,
                request: &admitted.request,
                spend: &spend,
                evaluation: &clear(),
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");

    assert_eq!(outcome, Placement::Sent { outcome: VenueOutcome::Accepted });
    assert_eq!(venue.submissions, 1);
    assert_eq!(
        drawn(&mut path, &Scope::Portfolio),
        Budget::Remaining(Amount::from_scaled(700_000_000)),
        "the portfolio was drawn by exactly what the order cost"
    );
}

/// A fresh engine has acted on nothing, and position zero is a real position.
#[tokio::test]
async fn a_fresh_engine_resumes_from_the_first_proposal() {
    let r = root("resume-fresh");
    let path = OrderPath::open(&r, "control", "limits").expect("open");

    assert_eq!(path.resume_from(), 0);
}

/// After an order is placed, the engine resumes past the proposal that caused it.
#[tokio::test]
async fn placing_an_order_advances_the_mark() {
    let r = root("resume-placed");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 3,
            request: &order(),
            spend: &spend(),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works");

    drop(path);
    let reopened = OrderPath::open(&r, "control", "limits").expect("reopen");
    assert_eq!(reopened.resume_from(), 4, "a restart must not place the same order a second time");
}

/// A refusal advances the mark exactly as an intent does, so a retry cannot record it twice.
#[tokio::test]
async fn a_refusal_advances_the_mark_too() {
    let r = root("resume-refused");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");

    // Halted, so the decision is refused rather than sent.
    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 9,
            request: &order(),
            spend: &spend(),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works");

    drop(path);
    assert_eq!(OrderPath::open(&r, "control", "limits").expect("reopen").resume_from(), 10);
}

/// The mark never moves backwards.
#[test]
fn the_mark_takes_the_highest_position_seen() {
    use bijective_event_store::projection::Projection;

    let mut progress = crate::progress::Progress::default();
    for position in [4u64, 9, 2] {
        progress
            .apply(&ControlEvent::Refused {
                decision_id: "d".into(),
                reason: RefusalReason::LimitsUnrecovered,
                from_position: position,
                at_ns: 1,
            })
            .expect("apply");
    }

    assert_eq!(progress.through(), Some(9));
    assert_eq!(progress.resume_from(), 10);
}

/// A settlement belongs to a decision already marked, and does not advance anything.
#[test]
fn a_settlement_does_not_advance_the_mark() {
    use bijective_event_store::projection::Projection;

    let mut progress = crate::progress::Progress::default();
    progress
        .apply(&ControlEvent::Settled {
            client_id: "abc123".into(),
            outcome: VenueOutcome::Accepted,
            at_ns: 1,
        })
        .expect("apply");

    assert_eq!(
        progress.through(),
        None,
        "crediting progress to a record that names no proposal would skip one"
    );
}

use bijective_event_store::store::StreamWriter;
use bijective_model::Proposal;
use bijective_risk::desks::{Mandate, MandateBook};

use crate::loop_::{Judgement, Reachable, drive};

fn write_proposals(root: &std::path::Path, proposals: &[Proposal]) {
    let mut w = StreamWriter::open(root, "proposals").expect("open");
    for proposal in proposals {
        w.append(&bijective_serialization::signal::encode(proposal).expect("encode"))
            .expect("append");
    }
}

fn mandate_book(maximum: i128) -> MandateBook {
    let mut instruments = std::collections::BTreeSet::new();
    instruments.insert(InstrumentId::new("aaa"));
    let mut book = MandateBook::new();
    book.declare(
        "alpha",
        Mandate {
            instruments,
            venue_preference: vec!["venue-a".into()],
            maximum_per_order: Amount::from_scaled(maximum),
            price_collar_bps: 500,
        },
    );
    book
}

fn signal(client_id: &str, quantity_scaled: i128) -> Proposal {
    Proposal {
        desk: "alpha".into(),
        instrument: InstrumentId::new("aaa"),
        side: Side::Buy,
        quantity_scaled,
        price_scaled: Some(200),
        cap_scaled: None,
        client_id: client_id.into(),
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

/// A book with nothing in it: every capture records no snapshot.
fn empty_book() -> bijective_decision_capture::SnapshotBuffer {
    bijective_decision_capture::SnapshotBuffer::new(4, 5_000_000_000)
}

fn reachable(venue: &mut Venue) -> Reachable<'_> {
    let mut declarations = BTreeMap::new();
    declarations.insert("venue-a".to_string(), declaration());
    let mut adapters: BTreeMap<String, &mut dyn bijective_venue::contract::VenueExecution> =
        BTreeMap::new();
    adapters.insert("venue-a".to_string(), venue);
    Reachable { declarations, adapters }
}

/// Proposals on a stream become orders at a venue, in order, drawing budget as they go.
#[tokio::test]
async fn proposals_on_a_stream_become_orders_at_a_venue() {
    let r = root("pipeline");
    write_proposals(&r, &[signal("ord1", 1_500), signal("ord2", 1_500)]);

    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_000_000_000);
    path.resume(SafeState::Active);

    let acted = drive(
        &mut path,
        &mut reachable(&mut venue),
        &clock(),
        &r,
        ProposalWindow::whole("proposals"),
        &Judgement {
            resolved: &BTreeSet::new(),
            mandates: &mandate_book(1_000_000_000),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            book: &empty_book(),
            staleness_bound_ns: 5_000_000_000,
            marks: &marks_at_200(),
        },
    )
    .await
    .expect("the loop itself works");

    assert_eq!(acted.len(), 2);
    assert!(acted.iter().all(|a| matches!(a.placement, Placement::Sent { .. })));
    assert_eq!(venue.submissions, 2);
    assert_eq!(
        drawn(&mut path, &Scope::Portfolio),
        Budget::Remaining(Amount::from_scaled(400_000_000)),
        "two orders at 3.00 each, drawn cumulatively"
    );
    assert_eq!(path.resume_from(), 2);
    drop(path);
    let declared =
        journal(&r).iter().filter(|e| matches!(e, ControlEvent::InstrumentScales { .. })).count();
    assert_eq!(declared, 1, "an instrument's scales are declared once, not per order");
}

/// Order is not incidental: the second proposal is judged against what the first already spent.
#[tokio::test]
async fn the_second_proposal_is_judged_against_what_the_first_spent() {
    let r = root("cumulative");
    // Different sizes, so which one goes through shows the order they were acted on in.
    write_proposals(&r, &[signal("ord1", 1_500), signal("ord2", 2_000)]);

    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    // 3.00 then 4.00 does not fit both. The reverse order would send the 4.00 and refuse the 3.00.
    declare(&mut path, Scope::Portfolio, 500_000_000);
    path.resume(SafeState::Active);

    let acted = drive(
        &mut path,
        &mut reachable(&mut venue),
        &clock(),
        &r,
        ProposalWindow::whole("proposals"),
        &Judgement {
            resolved: &BTreeSet::new(),
            mandates: &mandate_book(1_000_000_000),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            book: &empty_book(),
            staleness_bound_ns: 5_000_000_000,
            marks: &marks_at_200(),
        },
    )
    .await
    .expect("the loop itself works");

    assert_eq!(acted[0].position, 0, "the first proposal is acted on first");
    assert!(matches!(acted[0].placement, Placement::Sent { .. }));
    assert_eq!(acted[1].position, 1);
    assert!(matches!(
        acted[1].placement,
        Placement::Refused { reason: RefusalReason::NoBudget { .. } }
    ));
    assert_eq!(venue.submissions, 1);
    assert_eq!(
        drawn(&mut path, &Scope::Portfolio),
        Budget::Remaining(Amount::from_scaled(200_000_000)),
        "3.00 was drawn, not 4.00 - the cheaper one came first because it was first"
    );
}

/// A restart does not place the same order a second time.
#[tokio::test]
async fn a_restart_does_not_reissue_what_was_already_placed() {
    let r = root("pipeline-restart");
    write_proposals(&r, &[signal("ord1", 1_500)]);

    let mut venue = Venue::answering(VenueOutcome::Accepted);
    {
        let mut path = OrderPath::open(&r, "control", "limits").expect("open");
        declare(&mut path, Scope::Portfolio, 1_000_000_000);
        path.resume(SafeState::Active);
        drive(
            &mut path,
            &mut reachable(&mut venue),
            &clock(),
            &r,
            ProposalWindow::whole("proposals"),
            &Judgement {
                resolved: &BTreeSet::new(),
                mandates: &mandate_book(1_000_000_000),
                evaluation: &clear(),
                position_held: PositionHeld::No,
                book: &empty_book(),
                staleness_bound_ns: 5_000_000_000,
                marks: &marks_at_200(),
            },
        )
        .await
        .expect("first run");
    }
    assert_eq!(venue.submissions, 1);

    let mut path = OrderPath::open(&r, "control", "limits").expect("reopen");
    path.resume(SafeState::Active);
    let again = drive(
        &mut path,
        &mut reachable(&mut venue),
        &clock(),
        &r,
        ProposalWindow::whole("proposals"),
        &Judgement {
            resolved: &BTreeSet::new(),
            mandates: &mandate_book(1_000_000_000),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            book: &empty_book(),
            staleness_bound_ns: 5_000_000_000,
            marks: &marks_at_200(),
        },
    )
    .await
    .expect("second run");

    assert!(again.is_empty(), "nothing was left to act on");
    assert_eq!(venue.submissions, 1, "the venue saw the order once");
}

/// A proposal this build cannot read is refused and recorded, never passed over.
#[tokio::test]
async fn an_unreadable_proposal_is_refused_rather_than_skipped() {
    let r = root("unreadable-proposal");
    {
        let mut w = StreamWriter::open(&r, "proposals").expect("open");
        w.append(b"not a proposal this build knows").expect("append");
        w.append(&bijective_serialization::signal::encode(&signal("ord2", 1_500)).expect("encode"))
            .expect("append");
    }

    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_000_000_000);
    path.resume(SafeState::Active);

    let acted = drive(
        &mut path,
        &mut reachable(&mut venue),
        &clock(),
        &r,
        ProposalWindow::whole("proposals"),
        &Judgement {
            resolved: &BTreeSet::new(),
            mandates: &mandate_book(1_000_000_000),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            book: &empty_book(),
            staleness_bound_ns: 5_000_000_000,
            marks: &marks_at_200(),
        },
    )
    .await
    .expect("the loop itself works");

    assert_eq!(acted[0].placement, Placement::Refused { reason: RefusalReason::Unreadable });
    assert!(
        matches!(acted[1].placement, Placement::Sent { .. }),
        "one unreadable entry does not stop the ones after it"
    );
    assert_eq!(path.resume_from(), 2);
}

/// A proposal the desk does not mandate is refused, carrying what the admission said.
#[tokio::test]
async fn an_unmandated_proposal_is_refused_in_the_admissions_own_words() {
    let r = root("unmandated");
    let mut wrong = signal("ord1", 1_500);
    wrong.instrument = InstrumentId::new("eth-perp");
    write_proposals(&r, &[wrong]);

    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let acted = drive(
        &mut path,
        &mut reachable(&mut venue),
        &clock(),
        &r,
        ProposalWindow::whole("proposals"),
        &Judgement {
            resolved: &BTreeSet::new(),
            mandates: &mandate_book(1_000_000_000),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            book: &empty_book(),
            staleness_bound_ns: 5_000_000_000,
            marks: &marks_at_200(),
        },
    )
    .await
    .expect("the loop itself works");

    match &acted[0].placement {
        Placement::Refused { reason: RefusalReason::NotAdmitted { detail } } => {
            // The admission's own words, not the derive's shape.
            assert!(detail.contains("mandate"), "detail was {detail}");
            assert!(detail.contains("eth-perp"), "the instrument is missing: {detail}");
            assert!(!detail.contains("InstrumentId"), "still the derive's shape: {detail}");
        }
        other => panic!("expected an admission refusal, got {other:?}"),
    }
    assert_eq!(venue.submissions, 0);
}

use bijective_authority::Role;
use bijective_serialization::signal::{Command, Instruction};

use crate::loop_::carry_out_commands;

fn write_commands(root: &std::path::Path, commands: &[Command]) {
    let mut w = StreamWriter::open(root, "commands").expect("open");
    for command in commands {
        w.append(&bijective_serialization::signal::command::encode(command).expect("encode"))
            .expect("append");
    }
}

fn asked(id: &str, role: Role, instruction: Instruction) -> Command {
    Command { id: id.into(), actor: "duty".into(), role, instruction, at_ns: 1 }
}

/// An operator resumes trading, and the state recorded is the one read back.
#[test]
fn an_operator_resumes_and_the_platform_says_so() {
    let r = root("resume-command");
    write_commands(&r, &[asked("cmd-1", Role::Operator, Instruction::Resume)]);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");

    let carried = carry_out_commands(&mut path, &clock(), &r, "commands").expect("carry");

    assert_eq!(carried.len(), 1);
    assert_eq!(carried[0].state, SafeState::Active);
    assert_eq!(path.trading_state(), SafeState::Active);
}

/// An order whose outcome is unknown may have executed. Resuming on top of it would trade on a
/// position the platform does not know. The resume is refused, recorded with the count, and the platform stays
/// halted until reconciliation settles the order.
#[tokio::test]
async fn resuming_is_refused_while_an_order_is_unknown() {
    let r = root("resume-unknown");
    let mut venue = Venue::answering(VenueOutcome::Unknown(Unknown::Timeout));
    {
        let mut path = OrderPath::open(&r, "control", "limits").expect("open");
        declare(&mut path, Scope::Portfolio, 5_000);
        path.resume(SafeState::Active);
        path.place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-1",
                venue: "venue-a",
                from_position: 0,
                request: &order(),
                spend: &spend(),
                evaluation: &clear(),
                position_held: PositionHeld::No,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");
    }

    write_commands(&r, &[asked("cmd-1", Role::Operator, Instruction::Resume)]);
    let mut path = OrderPath::open(&r, "control", "limits").expect("reopen");
    assert_eq!(path.orders().0.unknown, 1);
    let carried = carry_out_commands(&mut path, &clock(), &r, "commands").expect("carry");

    assert_eq!(carried[0].state, SafeState::Halted);
    drop(path);
    let recorded = journal(&r);
    match recorded.last() {
        Some(ControlEvent::CommandApplied { detail, .. }) => {
            assert!(detail.contains("1 order(s) unknown"), "{detail}");
        }
        other => panic!("expected the refused resume on record, got {other:?}"),
    }
}

/// The floor cannot resume, and the refusal is on the record.
#[test]
fn an_observer_cannot_resume_and_the_refusal_is_recorded() {
    let r = root("observer-resume");
    write_commands(&r, &[asked("cmd-1", Role::Observer, Instruction::Resume)]);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");

    let carried = carry_out_commands(&mut path, &clock(), &r, "commands").expect("carry");

    assert_eq!(carried[0].state, SafeState::Halted, "nothing was relaxed");
    drop(path);
    match journal(&r).as_slice() {
        [ControlEvent::CommandApplied { role, action, detail, .. }] => {
            assert_eq!(*role, Role::Observer);
            assert_eq!(*action, bijective_authority::Action::Resume);
            assert!(detail.contains("may not resume"), "{detail}");
        }
        other => panic!("expected one recorded command, got {other:?}"),
    }
}

/// Every role may stop the platform, including the floor.
#[test]
fn any_role_may_halt() {
    for role in [Role::Observer, Role::Operator, Role::RiskOfficer] {
        let r = root(&format!("halt-{}", role.as_str()));
        write_commands(
            &r,
            &[
                asked("cmd-1", Role::Operator, Instruction::Resume),
                asked("cmd-2", role, Instruction::Halt),
            ],
        );
        let mut path = OrderPath::open(&r, "control", "limits").expect("open");

        carry_out_commands(&mut path, &clock(), &r, "commands").expect("carry");

        assert_eq!(path.trading_state(), SafeState::Halted, "{role:?}");
    }
}

/// A command this engine does not carry out is recorded rather than ignored.
#[test]
fn a_command_for_another_component_is_recorded_as_not_carried_out() {
    let r = root("not-ours");
    write_commands(&r, &[asked("cmd-1", Role::RiskOfficer, Instruction::ChangeLimit)]);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");

    carry_out_commands(&mut path, &clock(), &r, "commands").expect("carry");

    drop(path);
    assert!(journal(&r).iter().any(|e| matches!(
        e,
        ControlEvent::CommandApplied { detail, .. } if detail.contains("not the Control Engine's")
    )));
}

/// A restart does not carry out a command a second time.
#[test]
fn a_restart_does_not_reissue_a_command() {
    let r = root("command-restart");
    write_commands(&r, &[asked("cmd-1", Role::Operator, Instruction::Resume)]);
    {
        let mut path = OrderPath::open(&r, "control", "limits").expect("open");
        carry_out_commands(&mut path, &clock(), &r, "commands").expect("carry");
    }

    let mut path = OrderPath::open(&r, "control", "limits").expect("reopen");
    let again = carry_out_commands(&mut path, &clock(), &r, "commands").expect("carry");

    assert!(again.is_empty(), "nothing was left to carry out");
    assert_eq!(
        path.trading_state(),
        SafeState::Halted,
        "a resume is not re-applied on restart; a node comes back halted"
    );
}

/// A command this build cannot read is recorded, and the ones after it still run.
#[test]
fn an_unreadable_command_is_recorded_rather_than_skipped() {
    let r = root("unreadable-command");
    {
        let mut w = StreamWriter::open(&r, "commands").expect("open");
        w.append(b"not a command this build knows").expect("append");
        w.append(
            &bijective_serialization::signal::command::encode(&asked(
                "cmd-2",
                Role::Operator,
                Instruction::Resume,
            ))
            .expect("encode"),
        )
        .expect("append");
    }
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");

    let carried = carry_out_commands(&mut path, &clock(), &r, "commands").expect("carry");

    assert_eq!(carried.len(), 2);
    assert_eq!(
        path.trading_state(),
        SafeState::Active,
        "one unreadable command does not stop the ones after it"
    );
}

/// The proposal mark and the command mark are separate streams.
#[test]
fn progress_on_commands_is_not_progress_on_proposals() {
    let r = root("two-marks");
    write_commands(&r, &[asked("cmd-1", Role::Operator, Instruction::Resume)]);
    write_proposals(&r, &[signal("ord1", 1_500)]);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");

    carry_out_commands(&mut path, &clock(), &r, "commands").expect("carry");

    assert_eq!(path.commands_from(), 1);
    assert_eq!(path.resume_from(), 0, "no proposal has been answered");
}

use bijective_decision_capture::{
    BookUpdate as CaptureUpdate, Instant, Resolution, SnapshotBuffer,
};

fn book_holding(observed_at_ns: i128) -> SnapshotBuffer {
    let mut book = SnapshotBuffer::new(4, 5_000_000_000);
    book.on_market_data(
        CaptureUpdate {
            instrument: "aaa".into(),
            best_bid_ticks: 199,
            best_bid_qty_lots: 10,
            best_ask_ticks: 201,
            best_ask_qty_lots: 12,
            observed_at: Instant::new(observed_at_ns, Resolution::Millisecond),
        },
        observed_at_ns,
    );
    book
}

async fn drive_with(
    r: &std::path::Path,
    venue: &mut Venue,
    book: &SnapshotBuffer,
    path: &mut OrderPath,
) {
    drive(
        path,
        &mut reachable(venue),
        &clock(),
        r,
        ProposalWindow::whole("proposals"),
        &Judgement {
            resolved: &BTreeSet::new(),
            mandates: &mandate_book(1_000_000_000),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            book,
            staleness_bound_ns: 5_000_000_000,
            marks: &marks_at_200(),
        },
    )
    .await
    .expect("the loop itself works");
}

/// A decision is stamped against the book as it stood, and the lag is on the record.
#[tokio::test]
async fn a_decision_is_stamped_against_the_book_it_was_made_on() {
    let r = root("captured");
    let mut proposal = signal("ord1", 1_500);
    proposal.decided_at_ns = 1_000;
    write_proposals(&r, &[proposal]);

    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_000_000_000);
    path.resume(SafeState::Active);

    drive_with(&r, &mut venue, &book_holding(400), &mut path).await;

    drop(path);
    match journal(&r).first() {
        Some(ControlEvent::Captured { book: Some(b), detail, decided_at_ns, .. }) => {
            assert_eq!(*decided_at_ns, 1_000);
            assert_eq!(b.best_bid_ticks, 199);
            assert_eq!(b.best_ask_ticks, 201);
            assert_eq!(b.capture_lag_ns, 600, "1_000 decided minus 400 observed");
            assert!(detail.is_empty());
        }
        other => panic!("expected a capture first, got {other:?}"),
    }
}

/// A venue stamps on its own clock, so evidence reaches back a margin before the first intent.
#[tokio::test]
async fn evidence_reaches_back_a_margin_before_the_first_intent() {
    let r = root("evidence-margin");
    write_proposals(&r, &[signal("ord1", 1_500)]);
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_000_000_000);
    path.resume(SafeState::Active);

    drive_with(&r, &mut venue, &book_holding(400), &mut path).await;

    let (routed, at_ns) = journal(&r)
        .into_iter()
        .find_map(|event| match event {
            ControlEvent::Intent { venue: Some(routed), at_ns, .. } => Some((routed, at_ns)),
            _ => None,
        })
        .expect("an intent");
    assert_eq!(
        path.evidence_since_ns(&routed),
        Some(at_ns - crate::path::EVIDENCE_CLOCK_MARGIN_NS)
    );
}

/// The capture is written before the intent: the benchmark belongs to the decision.
#[tokio::test]
async fn the_capture_precedes_the_intent() {
    let r = root("capture-order");
    let mut proposal = signal("ord1", 1_500);
    proposal.decided_at_ns = 1_000;
    write_proposals(&r, &[proposal]);

    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_000_000_000);
    path.resume(SafeState::Active);

    drive_with(&r, &mut venue, &book_holding(400), &mut path).await;

    drop(path);
    assert!(matches!(
        journal(&r).as_slice(),
        [
            ControlEvent::Captured { .. },
            ControlEvent::InstrumentScales { .. },
            ControlEvent::Intent { .. },
            ControlEvent::Settled { .. }
        ]
    ));
}

/// A decision with no book is recorded as having none. Absent, not zeroed.
#[tokio::test]
async fn a_decision_with_no_snapshot_says_so_rather_than_inventing_one() {
    let r = root("capture-none");
    write_proposals(&r, &[signal("ord1", 1_500)]);

    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_000_000_000);
    path.resume(SafeState::Active);

    drive_with(&r, &mut venue, &empty_book(), &mut path).await;

    drop(path);
    match journal(&r).first() {
        Some(ControlEvent::Captured { book: None, detail, .. }) => {
            assert!(detail.contains("NoSnapshot"), "{detail}");
        }
        other => panic!("expected a capture with no book, got {other:?}"),
    }
}

/// A snapshot newer than the decision is a clock inconsistency, and is refused as one.
#[tokio::test]
async fn a_snapshot_taken_after_the_decision_is_not_a_benchmark() {
    let r = root("capture-future");
    let mut proposal = signal("ord1", 1_500);
    proposal.decided_at_ns = 400;
    write_proposals(&r, &[proposal]);

    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_000_000_000);
    path.resume(SafeState::Active);

    drive_with(&r, &mut venue, &book_holding(9_000), &mut path).await;

    drop(path);
    match journal(&r).first() {
        Some(ControlEvent::Captured { book: None, detail, .. }) => {
            assert!(detail.contains("SnapshotAfterDecision"), "{detail}");
        }
        other => panic!("expected a refused capture, got {other:?}"),
    }
}

/// A capture does not advance the proposal mark. The intent beside it already does.
#[tokio::test]
async fn a_capture_does_not_credit_progress_twice() {
    let r = root("capture-mark");
    write_proposals(&r, &[signal("ord1", 1_500)]);

    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_000_000_000);
    path.resume(SafeState::Active);

    drive_with(&r, &mut venue, &book_holding(0), &mut path).await;

    assert_eq!(path.resume_from(), 1, "one proposal answered, once");
}

/// A venue refuses a cancel with a code this build cannot read. The order may have filled, so it
/// must reach reconciliation and must not return the reservation.
#[tokio::test]
async fn an_unreadable_refusal_of_a_cancel_is_not_the_same_as_nothing_happening() {
    let dir = root("withdraw-unresolved");
    let mut path = OrderPath::open(&dir, "control", "limits").expect("open");
    let mut venue = Venue::at(
        dir.clone(),
        VenueOutcome::Rejected(Rejection::Unclassified {
            venue_code: "110001: order not exists or too late to cancel".to_string(),
        }),
    );

    let symbol = VenueSymbol::new("BTCUSDT");
    let out = path.withdraw(&mut venue, &symbol, "bl-1", 40).await.expect("withdraw");

    assert!(out.unresolved, "the venue refused and did not say the order is still there");

    let written = journal(&dir);
    assert!(
        matches!(written.last(), Some(ControlEvent::Withdrawn { client_id, .. }) if client_id == "bl-1"),
        "the withdrawal is on disk: {written:?}"
    );
}

/// The same shape with an answer the platform understands.
#[tokio::test]
async fn a_cancel_the_venue_accepted_leaves_nothing_open() {
    let dir = root("withdraw-accepted");
    let mut path = OrderPath::open(&dir, "control", "limits").expect("open");
    let mut venue = Venue::at(dir.clone(), VenueOutcome::Accepted);

    let symbol = VenueSymbol::new("BTCUSDT");
    let out = path.withdraw(&mut venue, &symbol, "bl-2", 41).await.expect("withdraw");

    assert!(!out.unresolved);
    assert_eq!(out.outcome, VenueOutcome::Accepted);
}

/// A venue leaving and rejoining are both written to the stream, so the history is recoverable.
#[tokio::test]
async fn a_venue_leaving_and_rejoining_are_both_on_the_record() {
    let dir = root("participation");
    let mut path = OrderPath::open(&dir, "control", "limits").expect("open");

    path.record_participation(
        "venue-a",
        Participation::Withdrawn { reason: "its account could not be read".to_string() },
        10,
    )
    .expect("withdrawal");
    path.record_participation("venue-a", Participation::Participating, 20).expect("return");

    let written = journal(&dir);
    let seen: Vec<_> = written
        .iter()
        .filter_map(|e| match e {
            ControlEvent::VenueParticipation { venue, participation, at_ns } => {
                Some((venue.as_str(), participation.clone(), *at_ns))
            }
            _ => None,
        })
        .collect();

    assert_eq!(seen.len(), 2, "{written:?}");
    assert!(
        matches!(&seen[0].1, Participation::Withdrawn { reason } if reason.contains("account")),
        "the cause did not survive the round trip: {:?}",
        seen[0].1
    );
    assert_eq!(seen[1].1, Participation::Participating);
    assert_eq!((seen[0].2, seen[1].2), (10, 20), "both ends of the interval are needed");
}

#[test]
fn the_record_of_a_drive_carries_the_scope_that_caused_it() {
    let event = ControlEvent::SafeStateDriven {
        target: SafeState::Halted,
        reason: SafeStateReason::Breached(
            breach(ScopeLevel::Venue, LimitKind::GrossExposure).breach,
        ),
        arrived: true,
        at_ns: 99,
        scope: Some(Scope::Venue("venue-a".into())),
    };
    let bytes = crate::wire::encode(&event).expect("encode");
    assert_eq!(crate::wire::decode(&bytes).expect("decode"), event);
}

/// A version-1 payload read by a newer decoder is a truncation, not a fault.
/// Built by hand because no encoder can produce one.
#[test]
fn a_record_written_before_the_scope_existed_reads_as_absent_rather_than_failing() {
    use bijective_serialization::codec::{ClassId, Writer};

    let mut w = Writer::new(ClassId::SafeStateDriven, 1);
    w.u8(2); // halted
    w.u8(3); // the breached reason
    w.u8(2); // venue level
    w.u8(0); // gross exposure
    w.u8(0).i128(1_500); // observed, money
    w.u8(0).i128(1_000); // maximum, money
    w.u8(1); // arrived
    w.i128(99);
    let older = w.finish();

    match crate::wire::decode(&older) {
        Ok(ControlEvent::SafeStateDriven { scope, arrived, at_ns, .. }) => {
            assert_eq!(scope, None, "an absent field is not a scope of its own");
            assert!(arrived);
            assert_eq!(at_ns, 99);
        }
        Ok(other) => panic!("decoded as something else: {other:?}"),
        Err(e) => panic!("an older record was refused rather than read: {e}"),
    }
}

/// A snapshot carrying exactly the figures named.
fn seen(figures: Vec<(Scope, LimitKind, Threshold)>) -> ExposureSnapshot {
    ExposureSnapshot { observed_at_ns: 500, figures }
}

fn money(scaled: i128) -> Threshold {
    Threshold::Money(Amount::from_scaled(scaled))
}

/// Drive a safe state on a venue-level gross-exposure breach, observed at 1,500.
async fn drive_a_breach(path: &mut OrderPath, id: &str) {
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let breached = Evaluation {
        breaches: vec![breach(ScopeLevel::Venue, LimitKind::GrossExposure)],
        ..clear()
    };
    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id,
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &breached,
            position_held: PositionHeld::Yes,
            semantics: ReduceOnlySemantics::FillTimeClamp,
        },
    )
    .await
    .expect("the path itself works");
}

fn answers(dir: &std::path::Path) -> Vec<(Scope, LimitKind, Threshold, PostconditionOutcome)> {
    journal(dir)
        .into_iter()
        .filter_map(|e| match e {
            ControlEvent::SafeStatePostcondition { scope, kind, before, outcome, .. } => {
                Some((scope, kind, before, outcome))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_halt_that_held_is_answered_with_what_the_figure_fell_to() {
    let r = root("postcondition-held");
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);
    drive_a_breach(&mut path, "dec-1").await;

    let later = seen(vec![(Scope::Venue("venue-a".into()), LimitKind::GrossExposure, money(900))]);
    assert_eq!(path.answer_postconditions(&later, 700).expect("answer"), 1);

    drop(path);
    let recorded = answers(&r);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0].0, Scope::Venue("venue-a".into()));
    assert_eq!(recorded[0].2, money(1_500), "the before-figure came off the breach");
    assert_eq!(
        recorded[0].3,
        PostconditionOutcome::Held { after: money(900) },
        "a figure that fell was not recorded as having fallen"
    );
}

/// The case the verifier exists for: the instruction honoured in name and not in effect.
#[tokio::test]
async fn exposure_that_grew_after_a_halt_is_recorded_as_the_instruction_unhonoured() {
    let r = root("postcondition-grew");
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);
    drive_a_breach(&mut path, "dec-1").await;

    let later =
        seen(vec![(Scope::Venue("venue-a".into()), LimitKind::GrossExposure, money(1_900))]);
    path.answer_postconditions(&later, 700).expect("answer");

    drop(path);
    let recorded = answers(&r);
    assert_eq!(
        recorded[0].3,
        PostconditionOutcome::Increased { after: money(1_900) },
        "exposure grew after a halt and the record did not say so"
    );
}

/// Missing is not satisfied: a postcondition must not go quiet when a figure does.
#[tokio::test]
async fn a_figure_that_stopped_arriving_is_unverifiable_and_not_satisfied() {
    let r = root("postcondition-missing");
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);
    drive_a_breach(&mut path, "dec-1").await;

    // Another venue's figure arrived. The one that was halted did not.
    let later = seen(vec![(Scope::Venue("venue-b".into()), LimitKind::GrossExposure, money(10))]);
    path.answer_postconditions(&later, 700).expect("answer");

    drop(path);
    assert_eq!(
        answers(&r)[0].3,
        PostconditionOutcome::Unverifiable,
        "an absent figure was reported as the postcondition satisfied"
    );
}

#[tokio::test]
async fn a_postcondition_once_answered_is_not_asked_again() {
    let r = root("postcondition-once");
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);
    drive_a_breach(&mut path, "dec-1").await;

    let later = seen(vec![(Scope::Venue("venue-a".into()), LimitKind::GrossExposure, money(900))]);
    assert_eq!(path.answer_postconditions(&later, 700).expect("first"), 1);
    assert_eq!(
        path.answer_postconditions(&later, 800).expect("second"),
        0,
        "the same figure was answered twice, which would repeat once per pass for the process's life"
    );

    drop(path);
    assert_eq!(answers(&r).len(), 1);
}

/// A second drive on the same figure carries a newer observation, and that is the one to answer.
#[tokio::test]
async fn a_second_drive_on_the_same_figure_is_answered_against_the_later_one() {
    let r = root("postcondition-supersede");
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);
    drive_a_breach(&mut path, "dec-1").await;

    // The same figure, seen higher a moment later. Resumed first, or the second drive never happens.
    path.resume(SafeState::Active);
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut second = breach(ScopeLevel::Venue, LimitKind::GrossExposure);
    second.breach.observed = money(2_400);
    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-2",
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &Evaluation { breaches: vec![second], ..clear() },
            position_held: PositionHeld::Yes,
            semantics: ReduceOnlySemantics::FillTimeClamp,
        },
    )
    .await
    .expect("the path itself works");

    let later =
        seen(vec![(Scope::Venue("venue-a".into()), LimitKind::GrossExposure, money(2_000))]);
    assert_eq!(
        path.answer_postconditions(&later, 700).expect("answer"),
        1,
        "two drives about one figure have one answer between them"
    );

    drop(path);
    let recorded = answers(&r);
    assert_eq!(recorded[0].2, money(2_400), "the answer used the earlier observation");
    assert_eq!(
        recorded[0].3,
        PostconditionOutcome::Held { after: money(2_000) },
        "against the stale figure this would have read as a rise"
    );
}

/// The ordinary case, not a recovery path: the halt is in one process and the answer in the next.
#[tokio::test]
async fn a_drive_recorded_before_a_restart_is_still_answered() {
    let r = root("postcondition-restart");
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);
    drive_a_breach(&mut path, "dec-1").await;
    drop(path);

    let mut restarted = OrderPath::open(&r, "control", "limits").expect("reopen");
    let later = seen(vec![(Scope::Venue("venue-a".into()), LimitKind::GrossExposure, money(900))]);
    assert_eq!(
        restarted.answer_postconditions(&later, 700).expect("answer"),
        1,
        "an engine that restarted forgot it had halted"
    );
}

/// Nothing was measured, so there is nothing to compare.
#[tokio::test]
async fn a_state_driven_on_something_never_measured_waits_for_no_answer() {
    let r = root("postcondition-unmeasured");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);

    let unjudged = Evaluation { unjudged: vec![LimitKind::GrossExposure], ..clear() };
    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "dec-1",
            venue: "venue-a",
            from_position: 0,
            request: &order(),
            spend: &spend(),
            evaluation: &unjudged,
            position_held: PositionHeld::Yes,
            semantics: ReduceOnlySemantics::FillTimeClamp,
        },
    )
    .await
    .expect("the path itself works");

    let later = seen(vec![(Scope::Venue("venue-a".into()), LimitKind::GrossExposure, money(900))]);
    assert_eq!(
        path.answer_postconditions(&later, 700).expect("answer"),
        0,
        "a state driven on a figure never observed was given an invented postcondition"
    );
}

fn reduce(id: &str, side: Side, quantity_scaled: i128) -> OrderRequest {
    OrderRequest { client_id: id.into(), side, quantity_scaled, reduce_only: true, ..order() }
}

async fn place_reduction(path: &mut OrderPath, request: &OrderRequest) -> Placement {
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: &format!("dec-{}", request.client_id),
            venue: "venue-a",
            from_position: 0,
            request,
            spend: &spend(),
            evaluation: &clear(),
            position_held: PositionHeld::Yes,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works")
}

/// A reduce-only order must oppose what is held and, with the reductions already working, not
/// pass through flat: anything else would open or grow a position.
#[tokio::test]
async fn a_reduce_only_order_that_would_not_reduce_is_refused() {
    let r = root("reduce-judged");
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);
    let refused = |p: &Placement| {
        matches!(p, Placement::Refused { reason: RefusalReason::DoesNotReduce { .. }, .. })
    };

    assert!(
        refused(&place_reduction(&mut path, &reduce("r-0", Side::Buy, 5)).await),
        "nothing held"
    );
    hold_short(&mut path, 20);
    assert!(refused(&place_reduction(&mut path, &reduce("r-1", Side::Sell, 5)).await), "same side");
    assert!(
        refused(&place_reduction(&mut path, &reduce("r-2", Side::Buy, 25)).await),
        "crosses flat"
    );
    assert!(matches!(
        place_reduction(&mut path, &reduce("r-3", Side::Buy, 15)).await,
        Placement::Sent { .. }
    ));
    assert!(
        refused(&place_reduction(&mut path, &reduce("r-4", Side::Buy, 10)).await),
        "15 already working toward flat leaves 5"
    );
    assert!(matches!(
        place_reduction(&mut path, &reduce("r-5", Side::Buy, 5)).await,
        Placement::Sent { .. }
    ));
}

/// A reducing platform takes a reduction that clears, and nothing else.
#[tokio::test]
async fn a_reducing_platform_takes_only_a_reduction() {
    let r = root("reducing");
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    hold_short(&mut path, 20);
    path.resume(SafeState::Reducing);

    let opening = OrderRequest { client_id: "o-1".into(), ..order() };
    let not_active = place_reduction(&mut path, &opening).await;
    assert!(matches!(
        not_active,
        Placement::Refused { reason: RefusalReason::NotActive { .. }, .. }
    ));
    assert!(matches!(
        place_reduction(&mut path, &reduce("r-1", Side::Buy, 10)).await,
        Placement::Sent { .. }
    ));
}

async fn place_plain(path: &mut OrderPath, venue: &mut Venue, id: &str) -> Placement {
    let request = OrderRequest { client_id: id.into(), ..order() };
    path.place(
        venue,
        &clock(),
        &Decision {
            id,
            venue: "venue-a",
            from_position: 0,
            request: &request,
            spend: &spend(),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works")
}

/// A venue's order rate refuses the submit it has no room for, before the venue is called, and
/// the venue's own rate refusal closes the window.
#[tokio::test]
async fn the_order_rate_refuses_what_the_window_has_no_room_for() {
    let r = root("rate");
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);
    path.set_order_rate("venue-a", crate::rate::OrderRate { orders: 1, per_ns: i128::MAX / 4 });
    let mut venue = Venue::answering(VenueOutcome::Accepted);

    assert!(matches!(place_plain(&mut path, &mut venue, "o-1").await, Placement::Sent { .. }));
    let second = place_plain(&mut path, &mut venue, "o-2").await;
    assert!(matches!(second, Placement::Refused { reason: RefusalReason::RateLimited { .. } }));
    assert_eq!(journal(&r).iter().filter(|e| matches!(e, ControlEvent::Intent { .. })).count(), 1);

    let r = root("rate-venue");
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);
    path.set_order_rate("venue-a", crate::rate::OrderRate { orders: 100, per_ns: 1_000 });
    let mut refusing = Venue::answering(VenueOutcome::Rejected(Rejection::RateLimited {
        retry_after_ns: Some(i128::MAX / 4),
    }));
    place_plain(&mut path, &mut refusing, "o-1").await;
    let after = place_plain(&mut path, &mut refusing, "o-2").await;
    assert!(matches!(after, Placement::Refused { reason: RefusalReason::RateLimited { .. } }));
}

/// A halt reaches the orders already resting: the entry is cancelled once, the exit is left, and
/// the cancel is on the record.
#[tokio::test]
async fn a_halt_withdraws_the_resting_entry_once_and_leaves_the_exit() {
    let r = root("sweep");
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    place_plain(&mut path, &mut venue, "entry").await;
    let exit = OrderRequest { client_id: "exit".into(), reduce_only: true, ..order() };
    path.place(
        &mut venue,
        &clock(),
        &Decision {
            id: "exit",
            venue: "venue-a",
            from_position: 0,
            request: &exit,
            spend: &spend(),
            evaluation: &clear(),
            position_held: PositionHeld::Yes,
            semantics: ReduceOnlySemantics::PlacementPrecondition,
        },
    )
    .await
    .expect("the path itself works");

    let mut reachable = crate::Reachable {
        declarations: BTreeMap::from([("venue-a".to_string(), venue.declaration.clone())]),
        adapters: BTreeMap::from([("venue-a".to_string(), &mut venue as &mut dyn VenueExecution)]),
    };
    let active = crate::withdraw_resting(&mut path, &mut reachable, 10).await.expect("swept");
    assert!(active.withdrawn.is_empty(), "an active node withdraws nothing");

    path.resume(SafeState::Halted);
    let swept = crate::withdraw_resting(&mut path, &mut reachable, 11).await.expect("swept");
    assert_eq!(swept.withdrawn, vec![("entry".to_string(), VenueOutcome::Accepted)]);
    assert!(swept.unreachable.is_empty());
    let again = crate::withdraw_resting(&mut path, &mut reachable, 12).await.expect("swept");
    assert!(again.withdrawn.is_empty(), "a cancel already asked for is not asked again");

    drop(path);
    let withdrawn: Vec<_> = journal(&r)
        .into_iter()
        .filter_map(|e| match e {
            ControlEvent::Withdrawn { client_id, .. } => Some(client_id),
            _ => None,
        })
        .collect();
    assert_eq!(withdrawn, vec!["entry".to_string()]);
}

/// A long of `quantity` in `aaa` on venue-a, accepted by a risk officer.
fn hold_long(path: &mut OrderPath, quantity: i128) {
    let command = bijective_serialization::signal::Command {
        id: format!("long-{quantity}"),
        actor: "risk-desk".into(),
        role: bijective_authority::Role::RiskOfficer,
        instruction: Instruction::AdjustPosition {
            venue: "venue-a".into(),
            adjustment: bijective_serialization::signal::PositionAdjustment {
                instrument: "aaa".into(),
                side: Side::Buy,
                quantity_scaled: quantity,
                price_scaled: 2_654_375,
                reason: "held for the test".into(),
                owner: None,
            },
        },
        at_ns: 0,
    };
    path.carry_out(&command, None, 0, 0).expect("adjusted");
}

/// A spot asset or an outcome share is sold only from what the record holds there: a sell
/// beyond it is refused before the venue is called, and one within it is sent (decision 0002).
#[tokio::test]
async fn a_sell_beyond_the_holding_of_an_instrument_that_cannot_be_short_is_refused() {
    let r = root("uncovered");
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    venue.declaration = declaration_where(|i| i.kind = InstrumentKind::Spot);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    path.resume(SafeState::Active);
    hold_long(&mut path, 6);

    let sell = |quantity_scaled| OrderRequest { side: Side::Sell, quantity_scaled, ..order() };

    let outcome = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-sell",
                venue: "venue-a",
                from_position: 0,
                request: &sell(10),
                spend: &spend(),
                evaluation: &clear(),
                position_held: PositionHeld::Yes,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");
    assert_eq!(
        outcome,
        Placement::Refused {
            reason: RefusalReason::Uncovered { held_scaled: 6, working_scaled: 0 }
        }
    );
    assert_eq!(venue.submissions, 0, "nothing was sent");

    let covered = path
        .place(
            &mut venue,
            &clock(),
            &Decision {
                id: "dec-sell-2",
                venue: "venue-a",
                from_position: 1,
                request: &OrderRequest { client_id: "abc124".into(), ..sell(6) },
                spend: &spend(),
                evaluation: &clear(),
                position_held: PositionHeld::Yes,
                semantics: ReduceOnlySemantics::PlacementPrecondition,
            },
        )
        .await
        .expect("the path itself works");
    assert_eq!(covered, Placement::Sent { outcome: VenueOutcome::Accepted });
}

/// Decision 0002, rule 5: a resolved outcome admits no orders, and the refusal is recorded.
#[tokio::test]
async fn a_proposal_on_a_resolved_outcome_is_refused_and_recorded() {
    let r = root("resolved_outcome");
    write_proposals(&r, &[signal("ord1", 1_500)]);
    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_000_000_000);
    path.resume(SafeState::Active);
    let resolved: BTreeSet<InstrumentId> = [InstrumentId::new("aaa")].into();

    let acted = drive(
        &mut path,
        &mut reachable(&mut venue),
        &clock(),
        &r,
        ProposalWindow::whole("proposals"),
        &Judgement {
            resolved: &resolved,
            mandates: &mandate_book(1_000_000_000),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            book: &empty_book(),
            staleness_bound_ns: 5_000_000_000,
            marks: &marks_at_200(),
        },
    )
    .await
    .expect("the loop itself works");

    assert!(matches!(acted[0].placement, Placement::Refused { .. }), "{:?}", acted[0].placement);
    assert_eq!(venue.submissions, 0);
}

/// A fresh mark at the price every test proposal carries, so the collar admits it.
fn marks_at_200() -> BTreeMap<InstrumentId, bijective_risk::valuation::Mark> {
    let mark = bijective_risk::valuation::Mark { price_scaled: 200, observed_at_ns: 1_000 };
    BTreeMap::from([(InstrumentId::new("aaa"), mark)])
}

/// A limit price that crosses the mark by more than the desk's collar never reaches the venue,
/// and the refusal says why.
#[tokio::test]
async fn a_limit_price_beyond_the_collar_is_refused_before_the_venue() {
    let r = root("collar");
    let mut fat_finger = signal("ord1", 1_500);
    fat_finger.price_scaled = Some(2_000);
    write_proposals(&r, &[fat_finger]);

    let mut venue = Venue::answering(VenueOutcome::Accepted);
    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    declare(&mut path, Scope::Portfolio, 1_000_000_000_000);
    path.resume(SafeState::Active);

    let acted = drive(
        &mut path,
        &mut reachable(&mut venue),
        &clock(),
        &r,
        ProposalWindow::whole("proposals"),
        &Judgement {
            resolved: &BTreeSet::new(),
            mandates: &mandate_book(1_000_000_000_000),
            evaluation: &clear(),
            position_held: PositionHeld::No,
            book: &empty_book(),
            staleness_bound_ns: 5_000_000_000,
            marks: &marks_at_200(),
        },
    )
    .await
    .expect("the loop itself works");

    let Placement::Refused { reason: RefusalReason::NotAdmitted { detail } } = &acted[0].placement
    else {
        panic!("refused at admission, got {:?}", acted[0].placement);
    };
    assert!(detail.contains("crosses the mark 200"), "{detail}");
    assert_eq!(venue.submissions, 0);
}

/// A flatten carried out over a breached evaluation and a halted node still closes what the
/// record holds: one reduce-only market order on the other side, recorded as an intent first.
#[tokio::test]
async fn a_flatten_closes_what_the_record_holds_past_the_figures() {
    let r = root("flatten");
    let mut w = StreamWriter::open(&r, "control").expect("open");
    let request = OrderRequest { quantity_scaled: 20, price_scaled: Some(200), ..order() };
    let fill = bijective_lifecycle::Fill {
        fill_id: "f-1".into(),
        venue_order_id: Some("v".into()),
        side: Side::Buy,
        price_scaled: 200,
        quantity_scaled: 20,
        fee: None,
        liquidity: bijective_lifecycle::Liquidity::Maker,
        exchange_time: bijective_model::market::ExchangeTime::NotPublished,
        received_at_ns: 3,
    };
    let flatten = ControlEvent::CommandApplied {
        command_id: "cmd-1".into(),
        actor: "ana".into(),
        role: bijective_authority::Role::Operator,
        action: bijective_authority::Action::Flatten,
        state: SafeState::Halted,
        detail: String::new(),
        from_position: 0,
        at_ns: 4,
    };
    for event in [
        ControlEvent::Intent {
            decision_id: "dec-1".into(),
            venue: Some("venue-a".into()),
            owner: None,
            request,
            from_position: 0,
            at_ns: 1,
        },
        ControlEvent::Settled {
            client_id: "abc123".into(),
            outcome: VenueOutcome::Accepted,
            at_ns: 2,
        },
        ControlEvent::Filled { client_id: "abc123".into(), fill, at_ns: 3 },
        flatten,
    ] {
        w.append(&crate::wire::encode(&event).expect("encode")).expect("append");
    }
    drop(w);

    let mut path = OrderPath::open(&r, "control", "limits").expect("open");
    let mut venue = Venue::at(r.clone(), VenueOutcome::Accepted);
    let done = crate::flatten::flatten_held(&mut path, &mut reachable(&mut venue), &clock())
        .await
        .expect("recorded");

    assert_eq!(venue.submissions, 1);
    assert!(matches!(done.placed[0].1, Placement::Sent { .. }));
    let intent = venue.seen_at_submit.iter().rev().find_map(|e| match e {
        ControlEvent::Intent { decision_id, request, .. } => {
            Some((decision_id.clone(), request.clone()))
        }
        _ => None,
    });
    let (decision_id, request) = intent.expect("the intent was written before the submit");
    assert_eq!(decision_id, "flatten:cmd-1");
    assert_eq!((request.side, request.quantity_scaled), (Side::Sell, 20));
    assert!(request.reduce_only && request.price_scaled.is_none());
    assert_eq!(path.resume_from(), 1, "the flatten order moved no proposal mark");

    let again = crate::flatten::flatten_held(&mut path, &mut reachable(&mut venue), &clock())
        .await
        .expect("recorded");
    assert!(again.placed.is_empty(), "acted on once");
}

#[path = "release_tests.rs"]
mod release;

/// Scales recorded before layout 17 read with no identity and no quote currency.
#[test]
fn scales_recorded_before_the_identity_read_without_one() {
    let current = ControlEvent::InstrumentScales {
        venue: "venue-a".into(),
        instrument: VenueSymbol::new("aaa"),
        price_scale: 2,
        quantity_scale: 3,
        at_ns: 4,
        identity: None,
        quote_currency: None,
    };
    let bytes = crate::wire::encode(&current).expect("encode");
    assert_eq!(crate::wire::decode(&bytes).expect("decode"), current);
    let mut older = bytes.clone();
    older.truncate(older.len() - 2);
    older[2..4].copy_from_slice(&16u16.to_be_bytes());
    assert_eq!(crate::wire::decode(&older).expect("version 16"), current);
}
