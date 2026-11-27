// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;
use crate::store::StreamWriter;

// A counter is enough: the thing under test is the guard around a projection, not any projection.
#[derive(Debug, Default, PartialEq, Eq)]
struct Tally {
    seen: u32,
    total: i64,
}

#[derive(Debug, PartialEq, Eq)]
struct Refused(i64);

impl Projection for Tally {
    type Event = i64;
    type Error = Refused;

    fn decode(payload: &[u8]) -> Result<i64, Refused> {
        let raw: [u8; 8] = payload.try_into().map_err(|_| Refused(-1))?;
        Ok(i64::from_be_bytes(raw))
    }

    fn apply(&mut self, event: &i64) -> Result<(), Refused> {
        if *event == 13 {
            return Err(Refused(*event));
        }
        self.seen += 1;
        self.total += *event;
        Ok(())
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("bijective-projection-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch root");
    dir
}

fn write(root: &std::path::Path, stream: &str, values: &[i64]) {
    let mut writer = StreamWriter::open(root, stream).expect("open");
    for value in values {
        writer.append(&value.to_be_bytes()).expect("append");
    }
}

#[test]
fn an_unrecovered_projection_reads_as_nothing() {
    let held = Recovered::<Tally>::default();

    assert_eq!(held.recovery(), Recovery::Incomplete);
    assert!(held.state().is_none(), "a projection that was never rebuilt must not be readable");
}

#[test]
fn a_rebuild_over_a_stream_reports_the_position_it_reached() {
    let root = scratch("through");
    write(&root, "tally", &[4, 5, 6]);

    let held = Recovered::<Tally>::rebuild(&root, "tally").expect("rebuild");

    assert_eq!(held.recovery(), Recovery::Verified { through: Some(2) });
    assert_eq!(held.state(), Some(&Tally { seen: 3, total: 15 }));
}

#[test]
fn an_empty_history_is_verified_not_incomplete() {
    let root = scratch("empty");
    write(&root, "tally", &[]);

    let held = Recovered::<Tally>::rebuild(&root, "tally").expect("rebuild");

    // Having looked and found nothing is a different fact from not having looked.
    assert_eq!(held.recovery(), Recovery::Verified { through: None });
    assert_eq!(held.state(), Some(&Tally::default()));
}

#[test]
fn an_event_that_cannot_be_read_refuses_the_whole_rebuild() {
    let root = scratch("undecodable");
    // One writer for the whole stream: the store holds a single-writer lock and is right to.
    let mut writer = StreamWriter::open(&root, "tally").expect("open");
    writer.append(&7i64.to_be_bytes()).expect("append");
    writer.append(b"not eight bytes of integer").expect("append");
    writer.append(&8i64.to_be_bytes()).expect("append");
    drop(writer);

    let outcome = Recovered::<Tally>::rebuild(&root, "tally");

    // Not "skip it and carry on with 7 and 8" - that is a projection that is wrong and looks right.
    match outcome {
        Err(RebuildError::Undecodable { at, .. }) => assert_eq!(at, 1),
        other => panic!("expected a refusal at position 1, got {other:?}"),
    }
}

#[test]
fn a_projection_refusing_an_event_names_where_it_refused() {
    let root = scratch("rejected");
    write(&root, "tally", &[1, 13, 2]);

    match Recovered::<Tally>::rebuild(&root, "tally") {
        Err(RebuildError::Rejected { at, cause }) => {
            assert_eq!(at, 1);
            assert_eq!(cause, Refused(13));
        }
        other => panic!("expected a rejection at position 1, got {other:?}"),
    }
}

#[test]
fn advancing_before_recovery_is_refused() {
    let mut held = Recovered::<Tally>::default();

    assert_eq!(held.advance(&5), Err(AdvanceError::NotRecovered));
    assert!(held.state().is_none());
}

#[test]
fn advancing_after_recovery_moves_the_projection() {
    let root = scratch("advance");
    write(&root, "tally", &[10]);
    let mut held = Recovered::<Tally>::rebuild(&root, "tally").expect("rebuild");

    held.advance(&32).expect("advance");

    assert_eq!(held.state(), Some(&Tally { seen: 2, total: 42 }));
    // The position is what the *rebuild* verified. Consuming live does not restate history.
    assert_eq!(held.recovery(), Recovery::Verified { through: Some(0) });
}

#[test]
fn a_refusal_while_advancing_reaches_the_caller() {
    let root = scratch("advance-refused");
    write(&root, "tally", &[1]);
    let mut held = Recovered::<Tally>::rebuild(&root, "tally").expect("rebuild");

    assert_eq!(held.advance(&13), Err(AdvanceError::Rejected(Refused(13))));
}
