// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The supervisor's gates.

use std::collections::BTreeMap;

use crate::runs::{Observer, Research};

use super::*;

fn env() -> BTreeMap<String, String> {
    BTreeMap::new()
}

fn spec() -> RunSpec<Research> {
    RunSpec::<Research>::declare(
        "model",
        vec![],
        env(),
        Some(4_102),
        crate::runs::Exchange::Stepwise,
    )
    .expect("declare")
}

fn completed(exit: Exit, stdout: &[u8]) -> Completed {
    Completed {
        exit,
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
        elapsed: Duration::from_millis(1),
    }
}

/// Three answers because the remedies differ: a package, a kernel setting, a delegated directory.
#[test]
fn each_missing_precondition_says_what_is_missing() {
    assert!(assess(Ok(())).is_ready());

    let cases = [
        Missing::UnshareAbsent,
        Missing::UserNamespaces,
        Missing::CgroupSubtree { detail: "permission denied".into() },
    ];

    let mut seen = Vec::new();
    for missing in cases {
        let outcome = assess(Err(missing));
        assert!(!outcome.is_ready(), "a host that cannot bound a run must not start");
        seen.push(outcome.detail().to_string());
    }

    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 3, "three preconditions, three distinguishable answers");
}

#[test]
fn a_ready_supervisor_has_started_nothing() {
    assert!(assess(Ok(())).detail().contains("none started"));
}

#[test]
fn a_run_that_was_cut_short_is_still_recorded() {
    let supervised = supervise(&spec(), &completed(Exit::Timeout, b"partial"), 10, 20);

    assert_eq!(supervised.entry.exit, ExitReason::Timeout);
    assert_eq!(supervised.entry.class, "research");
    assert_eq!(supervised.entry.started_ns, 10);
    assert_eq!(supervised.entry.ended_ns, 20);
}

/// A partial pass read as a result is how an underpowered verdict becomes a promotion.
#[test]
fn a_cut_short_run_hands_on_nothing() {
    for exit in [Exit::Timeout, Exit::LimitExceeded, Exit::Completed { status: 1 }] {
        let supervised = supervise(&spec(), &completed(exit.clone(), b"partial"), 10, 20);
        assert!(supervised.output.is_none(), "output was handed on for {exit:?}");
        assert_ne!(
            supervised.entry.emitted,
            sha256(&[]),
            "the record still says what it emitted, even though nothing may use it"
        );
    }
}

#[test]
fn a_clean_run_hands_on_what_it_emitted() {
    let supervised = supervise(&spec(), &completed(Exit::Completed { status: 0 }, b"result"), 1, 2);

    assert_eq!(supervised.output.as_deref(), Some(&b"result"[..]));
    assert_eq!(supervised.entry.emitted, sha256(b"result"));
}

#[test]
fn a_replay_that_matches_is_identical() {
    let first = supervise(&spec(), &completed(Exit::Completed { status: 0 }, b"same"), 1, 2);
    let again = supervise(&spec(), &completed(Exit::Completed { status: 0 }, b"same"), 3, 4);

    assert_eq!(compare(&first, &again), Reproduction::Identical);
}

#[test]
fn a_replay_that_differs_is_reported_rather_than_failed() {
    let first = supervise(&spec(), &completed(Exit::Completed { status: 0 }, b"one"), 1, 2);
    let again = supervise(&spec(), &completed(Exit::Completed { status: 0 }, b"two"), 3, 4);

    assert!(matches!(compare(&first, &again), Reproduction::Diverged { .. }));
}

/// A host problem reported as a divergence would become a finding about the hypothesis.
#[test]
fn a_replay_that_did_not_complete_compares_nothing() {
    let first = supervise(&spec(), &completed(Exit::Completed { status: 0 }, b"one"), 1, 2);
    let cut = supervise(&spec(), &completed(Exit::Timeout, b"one"), 3, 4);

    assert_eq!(compare(&first, &cut), Reproduction::NotCompared);
    assert_eq!(compare(&cut, &first), Reproduction::NotCompared);
}

#[test]
fn the_ledger_tells_the_classes_apart() {
    let research = supervise(&spec(), &completed(Exit::Completed { status: 0 }, b"x"), 1, 2);
    let watch =
        RunSpec::<Observer>::declare("model", vec![], env(), None, crate::runs::Exchange::Stepwise)
            .expect("declare");
    let observer = supervise(&watch, &completed(Exit::Completed { status: 0 }, b"x"), 1, 2);

    assert_ne!(research.entry.specification, observer.entry.specification);
    assert_eq!(observer.entry.class, "observer");
}

use crate::runs::Auditor;
use bijective_event_store::store::StreamWriter;

fn store(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bijective-extract-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    let mut w = StreamWriter::open(&dir, "market").expect("open");
    for value in 0u8..6 {
        w.append(&[value]).expect("append");
    }
    dir
}

fn extract_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bijective-inputs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Entries past the position are not there at all, which makes the bound a property of the run.
#[test]
fn a_research_extract_stops_at_the_declared_position() {
    let root = store("bounded");
    let into = extract_dir("bounded");
    let spec = RunSpec::<Research>::declare(
        "model",
        vec![],
        env(),
        Some(2),
        crate::runs::Exchange::Stepwise,
    )
    .expect("declare");

    let count = materialise(&spec, &root, "market", &into).expect("materialise");

    assert_eq!(count, 3, "positions 0, 1 and 2");
    let present: Vec<String> = std::fs::read_dir(&into)
        .expect("read")
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
        .collect();
    assert_eq!(present.len(), 3);
    assert!(
        !present.iter().any(|n| n.contains("00003")),
        "an entry past the window reached the run: {present:?}"
    );
}

#[test]
fn an_observer_extract_reaches_the_present() {
    let root = store("present");
    let into = extract_dir("present");
    let spec =
        RunSpec::<Observer>::declare("watch", vec![], env(), None, crate::runs::Exchange::Stepwise)
            .expect("declare");

    assert_eq!(materialise(&spec, &root, "market", &into).expect("materialise"), 6);
}

#[test]
fn a_different_position_is_a_different_extract() {
    let root = store("moved");
    let early = RunSpec::<Research>::declare(
        "model",
        vec![],
        env(),
        Some(1),
        crate::runs::Exchange::Stepwise,
    )
    .expect("declare");
    let later = RunSpec::<Research>::declare(
        "model",
        vec![],
        env(),
        Some(4),
        crate::runs::Exchange::Stepwise,
    )
    .expect("declare");

    let a = materialise(&early, &root, "market", &extract_dir("moved-a")).expect("materialise");
    let b = materialise(&later, &root, "market", &extract_dir("moved-b")).expect("materialise");

    assert_eq!((a, b), (2, 5));
    assert_ne!(early.identity(), later.identity());
}

#[test]
fn an_auditor_is_refused_a_market_extract() {
    let root = store("auditor");
    let spec =
        RunSpec::<Auditor>::declare("audit", vec![], env(), None, crate::runs::Exchange::Stepwise)
            .expect("declare");

    assert!(matches!(
        materialise(&spec, &root, "market", &extract_dir("auditor")),
        Err(ExtractError::NotAMarketReader)
    ));
}

use crate::protocol::Batch;
use crate::pump::{admit_lines, admit_result, window};

/// Written, not forged: the token exists only where an entry does.
fn recorded(dir: &std::path::Path) -> crate::ledger::Recorded {
    let mut ledger = StreamWriter::open(dir, crate::ledger::RUNS).expect("ledger");
    let entry = crate::runs::RunLedgerEntry {
        specification: bijective_event_store::digest::Digest([3u8; 32]),
        class: "research".to_string(),
        started_ns: 1,
        ended_ns: 2,
        exit: crate::runs::ExitReason::Completed { status: 0 },
        emitted: bijective_event_store::digest::Digest([4u8; 32]),
        strategy: Some(bijective_event_store::digest::Digest([5u8; 32])),
        program: None,
        content: None,
    };
    crate::ledger::record(&entry, &mut ledger).expect("record")
}
use bijective_event_store::store::read_range;
use bijective_serialization::signal::market::BookUpdate;

fn update(price: i128) -> BookUpdate {
    BookUpdate {
        instrument: bijective_model::InstrumentId::new("btc-perp"),
        seq: bijective_model::Sequence::Numbered(1),
        venue_ts_ms: 1_788_000_000_000,
        received_at_ns: None,
        bid_price: price,
        bid_quantity: 1_500,
        ask_price: price + 125,
        ask_quantity: 900,
        bid_depth: 5,
        ask_depth: 5,
    }
}

fn emitted(price: &str) -> String {
    format!(
        r#"{{"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":"1500","price_scaled":"{price}","client_id":"abc123","decided_at_ns":"1788000000000000000"}}"#
    )
}

/// An empty directory. The `store` fixture above seeds a market stream, which these do not want.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bijective-pump-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a place to write");
    dir
}

#[test]
fn a_window_ends_with_the_line_that_says_it_has() {
    let bytes = window(&[(0, update(2_654_375)), (1, update(2_654_500))], None).expect("render");
    let text = String::from_utf8(bytes).expect("utf-8");
    let lines: Vec<&str> = text.lines().collect();

    assert_eq!(lines.len(), 3, "two updates and the end: {text}");
    assert!(lines[0].contains(r#""bid_price":"2654375""#), "{}", lines[0]);
    assert_eq!(lines[2], r#"{"event":"end_of_window"}"#);
}

#[test]
fn what_a_strategy_emitted_reaches_the_proposal_stream() {
    let dir = scratch("admitted");
    let mut w = StreamWriter::open(&dir, "proposals").expect("writer");

    let out = format!("{}\n{}\n", emitted("2654375"), emitted("2654500"));
    let (admitted, refused) =
        admit_lines(out.lines(), &admitting(&sent(), Some("momentum")), &mut w).expect("admit");

    assert_eq!(admitted.len(), 2);
    assert!(refused.is_empty(), "{refused:?}");
    drop(w);

    let written = read_range(&dir, "proposals", 0, u64::MAX).expect("range");
    assert_eq!(written.len(), 2, "the supervisor is the only writer, and it wrote both");
    let first = bijective_serialization::signal::decode(&written[0].payload).expect("decode");
    assert_eq!(first.price_scaled, Some(2_654_375));
    assert_eq!(first.desk, "alpha");
    assert_eq!(first.strategy.as_deref(), Some("momentum"), "the supervisor names the strategy");
}

fn sent() -> crate::protocol::Sent {
    crate::protocol::Sent::from(Batch::of(0, 3))
}

fn admitting<'a>(
    sent: &'a crate::protocol::Sent,
    strategy: Option<&'a str>,
) -> crate::pump::Admitting<'a> {
    crate::pump::Admitting { sent, placeable: true, strategy }
}

/// The name comes from the supervisor: a line that names a strategy cannot make its proposal
/// count against another strategy's limits.
#[test]
fn a_strategy_cannot_name_itself_another_strategy() {
    let dir = scratch("impersonated");
    let mut w = StreamWriter::open(&dir, "proposals").expect("writer");

    let claiming =
        emitted("1").replace(r#""desk":"alpha""#, r#""desk":"alpha","strategy":"other""#);
    let held = sent();
    admit_lines(format!("{claiming}\n").lines(), &admitting(&held, Some("momentum")), &mut w)
        .expect("admit");
    drop(w);

    for entry in read_range(&dir, "proposals", 0, u64::MAX).expect("range") {
        let proposal = bijective_serialization::signal::decode(&entry.payload).expect("decode");
        assert_eq!(proposal.strategy.as_deref(), Some("momentum"));
    }
}

/// Refused, never skipped: a line this build cannot read may be the proposal that mattered.
#[test]
fn a_line_the_supervisor_cannot_read_is_named_rather_than_passed_over() {
    let dir = scratch("refused");
    let mut w = StreamWriter::open(&dir, "proposals").expect("writer");

    let out = format!("{}\nthis is not a proposal\n{}\n", emitted("1"), emitted("2"));
    let (admitted, refused) =
        admit_lines(out.lines(), &admitting(&sent(), Some("momentum")), &mut w).expect("admit");

    assert_eq!(admitted.len(), 2, "the readable lines were still carried");
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].line, 2, "an operator is told which line, counted as they would");
}

#[test]
fn a_proposal_the_protocol_refuses_does_not_reach_the_stream() {
    let dir = scratch("uncapped");
    let mut w = StreamWriter::open(&dir, "proposals").expect("writer");

    let uncapped = emitted("1").replace(r#","price_scaled":"1""#, "");
    let (admitted, refused) =
        admit_lines(format!("{uncapped}\n").lines(), &admitting(&sent(), None), &mut w)
            .expect("admit");

    assert_eq!(admitted.len(), 0);
    assert_eq!(refused.len(), 1);
    drop(w);
    assert!(read_range(&dir, "proposals", 0, u64::MAX).expect("range").is_empty());
}

/// A buffered writer ends output with a newline. Refusing it would refuse every well-behaved run.
#[test]
fn a_blank_line_is_not_a_refusal() {
    let dir = scratch("blank");
    let mut w = StreamWriter::open(&dir, "proposals").expect("writer");

    let (admitted, refused) =
        admit_lines(format!("{}\n\n", emitted("1")).lines(), &admitting(&sent(), None), &mut w)
            .expect("admit");

    assert_eq!(admitted.len(), 1);
    assert!(refused.is_empty(), "{refused:?}");
}

/// A real program in a real sandbox and store, no doubles.
#[test]
fn a_strategy_in_the_sandbox_reads_its_window_and_its_proposals_are_admitted() {
    use bijective_sandbox::{Limits, RunLayout, run};
    use std::time::Duration;

    let dir = scratch("end-to-end");
    let bound =
        Limits { memory_bytes: 64 * 1024 * 1024, processes: 32, wall: Duration::from_secs(20) };
    // Named by the operator: a cgroup fabricated in a temp directory bounds nothing.
    let Some(root) = std::env::var_os("BIJECTIVE_CGROUP_ROOT").map(std::path::PathBuf::from) else {
        return;
    };
    let Ok(cgroup) = bijective_sandbox::cgroup::RunCgroup::create(&root, "strategy", bound) else {
        // The host cannot bound a run: `probe` refuses that at startup, and this test must not report a pass it did not earn.
        return;
    };

    // Emits a proposal priced at the book count, so the assertion fails if the window came short, long or not at all.
    let strategy = dir.join("strategy.sh");
    std::fs::write(&strategy, r#"#!/bin/sh
n=0
while IFS= read -r line; do
  case "$line" in
    *'"event":"book"'*) n=$((n+1)) ;;
    *end_of_window*) break ;;
  esac
done
printf '{"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"%s","client_id":"c","decided_at_ns":"1"}\n' "$n"
"#).expect("write strategy");

    let completed = run(
        &cgroup,
        &RunLayout {
            inputs: dir.clone(),
            program: std::path::PathBuf::from("/bin/sh"),
            args: vec![strategy.display().to_string()],
            environment: std::collections::BTreeMap::new(),
        },
        bound,
        &window(&[(0, update(1)), (1, update(2)), (2, update(3))], None).expect("window"),
    )
    .expect("run");

    let entry = recorded(&dir);
    let mut w = StreamWriter::open(&dir, "proposals").expect("writer");
    let (admitted, refused) =
        admit_result(&completed.stdout, &entry, &admitting(&sent(), None), &mut w).expect("admit");
    assert_eq!(admitted.len(), 1, "stderr: {}", String::from_utf8_lossy(&completed.stderr));
    assert!(refused.is_empty(), "{refused:?}");
    drop(w);

    let written = read_range(&dir, "proposals", 0, u64::MAX).expect("range");
    let proposal = bijective_serialization::signal::decode(&written[0].payload).expect("decode");
    assert_eq!(
        proposal.price_scaled,
        Some(3),
        "the strategy did not receive the three book lines its window held"
    );
}

use crate::session::{SentThrough, SessionMark, decode as decode_mark, encode as encode_mark};

fn digest_of(byte: u8) -> bijective_event_store::digest::Digest {
    bijective_event_store::digest::Digest([byte; 32])
}

#[test]
fn a_session_mark_reads_back_as_what_was_written() {
    let mark = SessionMark {
        strategy: digest_of(7),
        market_stream: Some("market-venue-a".into()),
        sent_through: 4_102,
        at_ns: 1_788_000_000_000_000_000,
    };

    let payload = encode_mark(&mark).expect("encode");
    let read = decode_mark(&payload).expect("decode");

    assert_eq!(read, mark);
}

/// A newer layout is refused rather than read as a shorter one.
#[test]
fn a_mark_from_a_newer_build_is_refused() {
    use bijective_serialization::codec::{ClassId, Writer};

    let mut w = Writer::new(ClassId::StrategySessionMark, crate::session::VERSION + 1);
    w.bytes(&[0u8; 32]).expect("bytes");
    w.u64(1);
    w.i128(1);

    let read = decode_mark(&w.finish());

    assert!(read.is_err(), "a version this build does not know must be refused: {read:?}");
}

/// The furthest, not the last: a mark that could move backwards would understate a gap.
#[test]
fn the_progress_a_restart_reads_is_the_furthest_not_the_latest() {
    use bijective_event_store::projection::Projection;

    let mut sent = SentThrough::default();
    sent.apply(&SessionMark {
        strategy: digest_of(1),
        market_stream: Some("market-a".into()),
        sent_through: 900,
        at_ns: 2,
    })
    .expect("apply");
    sent.apply(&SessionMark {
        strategy: digest_of(1),
        market_stream: Some("market-a".into()),
        sent_through: 100,
        at_ns: 3,
    })
    .expect("apply");

    assert_eq!(sent.of(&digest_of(1), "market-a", true), Some(900));
}

#[test]
fn one_strategy_does_not_resume_on_another_ones_mark() {
    use bijective_event_store::projection::Projection;

    let mut sent = SentThrough::default();
    sent.apply(&SessionMark {
        strategy: digest_of(1),
        market_stream: Some("market-a".into()),
        sent_through: 900,
        at_ns: 2,
    })
    .expect("apply");

    assert_eq!(
        sent.of(&digest_of(2), "market-a", true),
        None,
        "a strategy with no mark of its own has none"
    );
}

/// Positions on two streams are not comparable, so each stream keeps its own mark. A version 1
/// mark names no stream: a session of one stream takes it, a session of several does not.
#[test]
fn a_mark_is_kept_per_stream_and_an_unnamed_one_serves_only_a_single_stream() {
    use bijective_event_store::projection::Projection;
    use bijective_serialization::codec::{ClassId, Writer};

    let mut sent = SentThrough::default();
    sent.apply(&SessionMark {
        strategy: digest_of(1),
        market_stream: Some("market-a".into()),
        sent_through: 40,
        at_ns: 1,
    })
    .expect("apply");
    let mut w = Writer::new(ClassId::StrategySessionMark, 1);
    w.bytes(&[1u8; 32]).expect("bytes");
    w.u64(900);
    w.i128(2);
    let unnamed = decode_mark(&w.finish()).expect("a version 1 mark");
    assert_eq!(unnamed.market_stream, None);
    sent.apply(&unnamed).expect("apply");

    assert_eq!(sent.of(&digest_of(1), "market-a", false), Some(40));
    assert_eq!(sent.of(&digest_of(1), "market-b", true), Some(900));
    assert_eq!(sent.of(&digest_of(1), "market-b", false), None);
}

use crate::ledger::{Ledger, RUNS, record};

fn entry_of(spec: u8, exit: crate::runs::ExitReason) -> crate::runs::RunLedgerEntry {
    crate::runs::RunLedgerEntry {
        specification: bijective_event_store::digest::Digest([spec; 32]),
        class: "research".to_string(),
        started_ns: 1_788_000_000_000_000_000,
        ended_ns: 1_788_000_000_500_000_000,
        exit,
        emitted: bijective_event_store::digest::Digest([9u8; 32]),
        strategy: Some(bijective_event_store::digest::Digest([8u8; 32])),
        program: None,
        content: None,
    }
}

/// Every way a run can end survives the wire, including the ones that carry no status.
#[test]
fn a_run_entry_reads_back_as_what_was_written() {
    use crate::runs::ExitReason;

    for exit in [
        ExitReason::Completed { status: 0 },
        ExitReason::Completed { status: -1 },
        ExitReason::LimitExceeded,
        ExitReason::Timeout,
        ExitReason::Refused,
    ] {
        let entry = entry_of(1, exit);
        let payload = crate::runs::ledger::encode(&entry).expect("encode");

        assert_eq!(crate::runs::ledger::decode(&payload).expect("decode"), entry);
    }
}

#[test]
fn a_run_entry_from_a_newer_build_is_refused() {
    use bijective_serialization::codec::{ClassId, Writer};

    let mut w = Writer::new(ClassId::StrategyRunRecorded, crate::runs::ledger::VERSION + 1);
    w.bytes(&[0u8; 32]).expect("bytes");

    assert!(crate::runs::ledger::decode(&w.finish()).is_err());
}

#[test]
fn a_run_recorded_in_one_process_is_found_by_the_next() {
    use crate::runs::ExitReason;
    use bijective_event_store::projection::Recovered;

    let dir = scratch("ledger");
    let mut w = StreamWriter::open(&dir, RUNS).expect("writer");
    record(&entry_of(1, ExitReason::Completed { status: 0 }), &mut w).expect("record");
    record(&entry_of(1, ExitReason::Timeout), &mut w).expect("record");
    record(&entry_of(2, ExitReason::LimitExceeded), &mut w).expect("record");
    drop(w);

    let held = Recovered::<Ledger>::rebuild(&dir, RUNS).expect("rebuild");
    let ledger = held.state().expect("rebuilt");

    let both = ledger.of(&bijective_event_store::digest::Digest([1u8; 32]));
    assert_eq!(both.len(), 2, "a replay leaves two entries, not one overwritten");
    assert_eq!(both[0].exit, ExitReason::Completed { status: 0 }, "in the order recorded");
    assert_eq!(both[1].exit, ExitReason::Timeout);
    assert_eq!(ledger.of(&bijective_event_store::digest::Digest([2u8; 32])).len(), 1);
    assert!(
        ledger.of(&bijective_event_store::digest::Digest([7u8; 32])).is_empty(),
        "a specification nothing recorded has no entries, which is not the same as a run that failed"
    );
}

#[test]
fn a_session_that_reads_the_present_is_not_asked_to_reproduce() {
    let entries = vec![entry_at("live", Digest([1u8; 32])), entry_at("live", Digest([2u8; 32]))];

    assert_eq!(reproduction_of("live", &entries), Reproduction::NotClaimed);
}

#[test]
fn a_bounded_run_is_still_asked() {
    let entries =
        vec![entry_at("research", Digest([1u8; 32])), entry_at("research", Digest([2u8; 32]))];

    assert!(matches!(reproduction_of("research", &entries), Reproduction::Diverged { .. }));
}

#[test]
fn a_class_this_build_cannot_name_is_asked_anyway() {
    let entries = vec![entry_at("something-later", Digest([1u8; 32]))];

    assert_eq!(reproduction_of("something-later", &entries), Reproduction::NotCompared);
}

fn entry_at(class: &str, emitted: Digest) -> crate::runs::RunLedgerEntry {
    crate::runs::RunLedgerEntry {
        specification: Digest([3u8; 32]),
        class: class.to_string(),
        started_ns: 1,
        ended_ns: 2,
        exit: crate::runs::ExitReason::Completed { status: 0 },
        emitted,
        strategy: Some(Digest([5u8; 32])),
        program: None,
        content: None,
    }
}

/// Both print the number. Only one says a person chose it, which is why the value stopped being a constant.
#[test]
fn a_tolerance_says_whether_anyone_declared_it() {
    use crate::Behind;

    assert_eq!(Behind::Declared(64).events(), 64);
    assert_eq!(Behind::NotDeclared(1024).events(), 1024);

    assert_eq!(format!("{}", Behind::Declared(64)), "64 event(s) behind, declared");
    assert_eq!(
        format!("{}", Behind::NotDeclared(1024)),
        "1024 event(s) behind, a bound not declared"
    );
}

/// A session opens by naming its venue and the fees declared for it. With none declared, the
/// fees are absent rather than zero, and a stream that names no venue opens with nothing.
#[test]
fn a_session_opens_with_its_venue_and_its_declared_fees() {
    use bijective_event_store::store::StreamWriter;
    use bijective_serialization::signal::fees::{FEES, Fees, encode};

    let store = std::env::temp_dir().join(format!("bl-opening-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&store);
    std::fs::create_dir_all(&store).expect("store");

    let bare = String::from_utf8(opening(&store, "market-venue-a").expect("read").expect("a line"))
        .expect("text");
    assert!(bare.contains(r#""venue":"venue-a""#) && !bare.contains("fee"), "{bare}");
    assert_eq!(opening(&store, "elsewhere").expect("read"), None);

    let declared = Fees {
        venue: "venue-a".into(),
        taker: 550_000_000,
        maker: -100_000_000,
        source: "venue fee page".into(),
        read_at_ns: 1,
    };
    let mut w = StreamWriter::open(&store, FEES).expect("open");
    w.append(&encode(&declared).expect("encode")).expect("append");
    let line = String::from_utf8(opening(&store, "market-venue-a").expect("read").expect("a line"))
        .expect("text");
    assert!(line.contains(r#""taker_fee_bps_scaled":"550000000""#), "{line}");
    assert!(line.contains(r#""maker_fee_bps_scaled":"-100000000""#), "{line}");
}

/// The increments a strategy must place on reach it before its first top.
#[test]
fn a_session_opens_with_the_declared_instrument_rules() {
    use bijective_event_store::store::StreamWriter;
    use bijective_serialization::signal::instruments::{
        InstrumentRules, VenueInstruments, encode, stream_for,
    };

    let store = std::env::temp_dir().join(format!("bl-opening-rules-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&store);
    std::fs::create_dir_all(&store).expect("store");
    let declared = VenueInstruments {
        venue: "venue-a".into(),
        instruments: vec![InstrumentRules {
            instrument: "btc-perp".into(),
            price_scale: 1,
            quantity_scale: 3,
            price_tick: 5,
            quantity_lot: 1,
            minimum_size: 1,
            minimum_notional: 50,
            trading: true,
            kind: Some("perpetual".into()),
            quote_currency: Some("usdt".into()),
            settlement_currency: Some("usdt".into()),
            base_currency: None,
        }],
    };
    let mut w = StreamWriter::open(&store, &stream_for("venue-a")).expect("open");
    w.append(&encode(&declared).expect("encode")).expect("append");
    let line = String::from_utf8(opening(&store, "market-venue-a").expect("read").expect("a line"))
        .expect("text");
    assert!(line.contains(r#""instrument":"btc-perp""#), "{line}");
    assert!(line.contains(r#""price_tick":"5""#) && line.contains(r#""trading":true"#), "{line}");
    assert!(line.contains(r#""kind":"perpetual""#), "{line}");
}
