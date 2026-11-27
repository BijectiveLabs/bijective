// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Replay gates for limit state: a restart must never restore spent budget.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use bijective_event_store::store::StreamWriter;
use bijective_limits::{Amount, LimitKind, Ratio, Scope, Threshold};
use bijective_serialization::codec::{ClassId, DecodeError, Writer};

use super::*;
use bijective_event_store::projection::{Projection, RebuildError};

use crate::limit_state::state::{Budget, Limits, Recovery};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn root(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "bijective-limit-state-{}-{}-{}",
        std::process::id(),
        name,
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn every_class() -> Vec<LimitEvent> {
    vec![
        LimitEvent::Declared {
            scope: Scope::Portfolio,
            kind: LimitKind::GrossExposure,
            maximum: Threshold::Money(Amount::from_scaled(1_000)),
            config_version: 7,
        },
        LimitEvent::Declared {
            scope: Scope::Venue("venue-a".into()),
            kind: LimitKind::Drawdown,
            maximum: Threshold::Ratio(Ratio::from_scaled(250)),
            config_version: 7,
        },
        LimitEvent::Consumed {
            scope: Scope::Desk("alpha".into()),
            kind: LimitKind::NetExposure,
            amount: Amount::from_scaled(-42),
            decision_id: "ord-9".into(),
        },
        LimitEvent::Released {
            scope: Scope::Instrument("BTCUSDT".into()),
            kind: LimitKind::Leverage,
            amount: Amount::from_scaled(0),
            reason: ReleaseReason::OrderCancelled,
            decision_id: None,
        },
        LimitEvent::Released {
            scope: Scope::Venue("venue-a".into()),
            kind: LimitKind::GrossExposure,
            amount: Amount::from_scaled(7),
            reason: ReleaseReason::OrderCancelled,
            decision_id: Some("ord-9".into()),
        },
    ]
}

/// Byte-identical replay is binding.
#[test]
fn every_class_round_trips_to_identical_bytes() {
    for event in every_class() {
        let first = encode(&event).expect("encodes");
        let decoded = decode(&first).expect("decodes");
        assert_eq!(decoded, event, "the value survived the round trip");
        let second = encode(&decoded).expect("re-encodes");
        assert_eq!(first, second, "re-encoding produced different bytes");
    }
}

#[test]
fn a_class_this_build_does_not_know_refuses_the_replay() {
    let payload = vec![0xde, 0xad, 0x00, 0x01];
    assert!(matches!(decode(&payload), Err(DecodeError::UnknownClass { found: 0xdead })));
}

#[test]
fn a_newer_class_version_refuses_the_replay() {
    let mut w = Writer::new(ClassId::LimitConsumed, VERSION + 1);
    w.u8(0);
    let payload = w.finish();
    assert!(matches!(decode(&payload), Err(DecodeError::UnsupportedVersion { .. })));
}

#[test]
fn a_restart_does_not_restore_consumed_budget() {
    let r = root("restart");
    let scope = Scope::Desk("alpha".into());

    {
        let mut w = StreamWriter::open(&r, "limits").unwrap();
        let mut state = LimitState::recover(&r, "limits").unwrap();
        state
            .declare(
                &mut w,
                scope.clone(),
                LimitKind::GrossExposure,
                Threshold::Money(Amount::from_scaled(1_000)),
                1,
            )
            .unwrap();
        state
            .consume(
                &mut w,
                scope.clone(),
                LimitKind::GrossExposure,
                Amount::from_scaled(400),
                "ord-1",
            )
            .unwrap();
        assert_eq!(
            state.budget(&scope, LimitKind::GrossExposure),
            Budget::Remaining(Amount::from_scaled(600))
        );
    }

    let restarted = LimitState::recover(&r, "limits").unwrap();
    assert!(restarted.may_trade());
    assert_eq!(
        restarted.budget(&scope, LimitKind::GrossExposure),
        Budget::Remaining(Amount::from_scaled(600)),
        "the restart restored a budget that had already been spent"
    );
}

/// Unrecovered is distinct from Exhausted: only one says the platform has not looked yet.
#[test]
fn an_unrecovered_state_fails_closed_and_says_which_kind_of_no() {
    let state = LimitState::new();
    assert_eq!(state.recovery(), Recovery::Incomplete);
    assert!(!state.may_trade(), "trading must not resume before the replay");
    assert_eq!(state.budget(&Scope::Portfolio, LimitKind::GrossExposure), Budget::Unrecovered);
}

#[test]
fn an_empty_stream_recovers_as_verified_and_empty() {
    let r = root("empty");
    let state = LimitState::recover(&r, "limits").unwrap();
    assert_eq!(state.recovery(), Recovery::Verified { through: None });
    assert!(state.may_trade());
    assert_eq!(
        state.budget(&Scope::Portfolio, LimitKind::GrossExposure),
        Budget::Undeclared,
        "verified and empty means no limit declared, not no answer"
    );
}

#[test]
fn the_consumption_is_durable_before_the_caller_could_act_on_it() {
    let r = root("record-first");
    let scope = Scope::Venue("venue-a".into());
    let mut w = StreamWriter::open(&r, "limits").unwrap();
    let mut state = LimitState::recover(&r, "limits").unwrap();

    state
        .declare(
            &mut w,
            scope.clone(),
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(500)),
            1,
        )
        .unwrap();
    let seq = state
        .consume(&mut w, scope.clone(), LimitKind::GrossExposure, Amount::from_scaled(500), "ord-2")
        .unwrap();

    // A second independent replay while the writer is open shows the append was durable.
    let seen = LimitState::recover(&r, "limits").unwrap();
    assert_eq!(seq, 1);
    assert_eq!(seen.budget(&scope, LimitKind::GrossExposure), Budget::Exhausted);
}

/// Fail closed: approving without recording the spend is the risk limits failure.
#[test]
fn a_consumption_that_cannot_be_encoded_spends_nothing() {
    let r = root("encode-fail");
    let scope = Scope::Portfolio;
    let mut w = StreamWriter::open(&r, "limits").unwrap();
    let mut state = LimitState::recover(&r, "limits").unwrap();
    state
        .declare(
            &mut w,
            scope.clone(),
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(1_000)),
            1,
        )
        .unwrap();

    let absurd = "x".repeat(bijective_serialization::codec::MAX_FIELD_LEN as usize + 1);
    let outcome = state.consume(
        &mut w,
        scope.clone(),
        LimitKind::GrossExposure,
        Amount::from_scaled(100),
        &absurd,
    );

    assert!(matches!(outcome, Err(StateError::Encode(_))));
    assert_eq!(
        state.budget(&scope, LimitKind::GrossExposure),
        Budget::Remaining(Amount::from_scaled(1_000)),
        "a refused consumption must not have spent anything"
    );
}

/// Run against the fold: `LimitState` folds nothing before recovery.
#[test]
fn a_release_that_exceeds_what_was_consumed_is_refused() {
    let mut state = Limits::default();
    let scope = Scope::Desk("alpha".into());
    state
        .apply(&LimitEvent::Consumed {
            scope: scope.clone(),
            kind: LimitKind::GrossExposure,
            amount: Amount::from_scaled(100),
            decision_id: "ord-3".into(),
        })
        .unwrap();

    let outcome = state.apply(&LimitEvent::Released {
        scope,
        kind: LimitKind::GrossExposure,
        amount: Amount::from_scaled(101),
        reason: ReleaseReason::PositionClosed,
        decision_id: None,
    });
    assert!(matches!(outcome, Err(StateError::ReleaseExceedsConsumed { .. })));
}

#[test]
fn a_corrupt_history_refuses_to_recover() {
    let r = root("corrupt");
    {
        let mut w = StreamWriter::open(&r, "limits").unwrap();
        let mut state = LimitState::recover(&r, "limits").unwrap();
        state
            .declare(
                &mut w,
                Scope::Portfolio,
                LimitKind::GrossExposure,
                Threshold::Money(Amount::from_scaled(1_000)),
                1,
            )
            .unwrap();
    }

    let seg = r.join("limits").join("000000000000.seg");
    let mut bytes = fs::read(&seg).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&seg, &bytes).unwrap();

    assert!(matches!(LimitState::recover(&r, "limits"), Err(RebuildError::Store(_))));
}

#[test]
fn a_ratio_limit_is_not_a_budget() {
    let r = root("ratio");
    let mut w = StreamWriter::open(&r, "limits").unwrap();
    let mut state = LimitState::recover(&r, "limits").unwrap();
    state
        .declare(
            &mut w,
            Scope::Portfolio,
            LimitKind::Drawdown,
            Threshold::Ratio(Ratio::from_scaled(250)),
            1,
        )
        .unwrap();

    assert_eq!(state.budget(&Scope::Portfolio, LimitKind::Drawdown), Budget::NotDrawable);
}

/// A reordered enum would reinterpret recorded events, so the bytes are asserted directly.
#[test]
fn scope_levels_are_pinned_to_their_wire_bytes() {
    let cases = [
        (Scope::Portfolio, 0u8),
        (Scope::Desk("d".into()), 1),
        (Scope::Venue("v".into()), 2),
        (Scope::Strategy("s".into()), 3),
        (Scope::Instrument("i".into()), 4),
    ];
    for (scope, code) in cases {
        let payload = encode(&LimitEvent::Consumed {
            scope: scope.clone(),
            kind: LimitKind::GrossExposure,
            amount: Amount::from_scaled(1),
            decision_id: "x".into(),
        })
        .unwrap();
        // envelope is four bytes. The scope level code is the fifth.
        assert_eq!(payload[4], code, "wire code moved for {scope:?}");
        assert_eq!(
            decode(&payload).unwrap(),
            LimitEvent::Consumed {
                scope,
                kind: LimitKind::GrossExposure,
                amount: Amount::from_scaled(1),
                decision_id: "x".into(),
            }
        );
    }
}

/// The registry is shared, so a market-data payload can arrive here. Refuse it before reading fields.
#[test]
fn a_payload_from_another_domain_is_refused_before_any_field_is_read() {
    let mut w = Writer::new(ClassId::MarketBookUpdate, 1);
    w.str("BTCUSDT").unwrap();
    w.u64(1).u64(2).i128(3).i128(4).i128(5).i128(6).u32(7).u32(8);

    let err = crate::limit_state::wire::decode(&w.finish()).unwrap_err();
    assert!(
        matches!(err, bijective_serialization::codec::DecodeError::UnknownClass { found } if found == ClassId::MarketBookUpdate.as_u16()),
        "expected a refused class, got {err:?}"
    );
}

/// Appending against an unread balance spends budget the platform cannot account for.
#[test]
fn a_write_before_recovery_is_refused() {
    let r = root("write-before-recovery");
    let mut w = StreamWriter::open(&r, "limits").unwrap();
    let mut state = LimitState::new();

    let outcome = state.consume(
        &mut w,
        Scope::Portfolio,
        LimitKind::GrossExposure,
        Amount::from_scaled(10),
        "ord-9",
    );

    assert!(matches!(outcome, Err(StateError::NotRecovered)));
}

/// A decision returns what it took and no more, and its holding reads back per scope.
#[test]
fn a_decision_returns_only_what_it_took() {
    let mut limits = Limits::default();
    let scope = Scope::Venue("venue-a".into());
    let consumed = |id: &str, n: i128| LimitEvent::Consumed {
        scope: scope.clone(),
        kind: LimitKind::GrossExposure,
        amount: Amount::from_scaled(n),
        decision_id: id.into(),
    };
    let released = |id: &str, n: i128| LimitEvent::Released {
        scope: scope.clone(),
        kind: LimitKind::GrossExposure,
        amount: Amount::from_scaled(n),
        reason: ReleaseReason::OrderCancelled,
        decision_id: Some(id.into()),
    };
    limits.apply(&consumed("ord-1", 100)).unwrap();
    limits.apply(&consumed("ord-2", 50)).unwrap();
    limits.apply(&released("ord-1", 60)).unwrap();
    // ord-2 took 50. Returning 60 under its name would hand back ord-1's budget.
    assert!(matches!(
        limits.apply(&released("ord-2", 60)),
        Err(StateError::ReleaseExceedsConsumed { .. })
    ));
}

/// A version-1 release, written before releases named a decision, still reads, naming none.
#[test]
fn a_release_written_before_decisions_were_named_reads_as_naming_none() {
    let mut w = Writer::new(ClassId::LimitReleased, 1);
    w.u8(0).str("").unwrap();
    w.u8(0).i128(5).u8(1);
    let event = decode(&w.finish());
    assert!(matches!(event, Ok(LimitEvent::Released { decision_id: None, .. })), "{event:?}");
}

/// A release the replay would refuse is refused before it is written: the stream it would have
/// poisoned still rebuilds, and the budget is unchanged.
#[test]
fn a_refused_release_never_reaches_the_stream() {
    let dir = std::env::temp_dir().join(format!("bl-limits-refused-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let scope = Scope::Venue("venue-a".into());
    let mut writer = StreamWriter::open(&dir, "limits").unwrap();
    let mut state = LimitState::recover(&dir, "limits").unwrap();
    state
        .declare(
            &mut writer,
            scope.clone(),
            LimitKind::GrossExposure,
            Threshold::Money(Amount::from_scaled(1_000)),
            1,
        )
        .unwrap();
    state
        .consume(
            &mut writer,
            scope.clone(),
            LimitKind::GrossExposure,
            Amount::from_scaled(100),
            "ord-1",
        )
        .unwrap();
    let before = writer.next_sequence();

    let refused = state.release(
        &mut writer,
        scope.clone(),
        LimitKind::GrossExposure,
        Amount::from_scaled(150),
        ReleaseReason::OrderCancelled,
        Some("ord-1"),
    );
    assert!(matches!(refused, Err(StateError::ReleaseExceedsConsumed { .. })));
    assert_eq!(writer.next_sequence(), before, "nothing was written");
    drop(writer);

    let rebuilt = LimitState::recover(&dir, "limits").expect("the stream still rebuilds");
    assert_eq!(
        rebuilt.budget(&scope, LimitKind::GrossExposure),
        Budget::Remaining(Amount::from_scaled(900))
    );
}
