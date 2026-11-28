// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Fault injection against the verifier.
//! Each test is one specific way the file can be wrong. Damage is applied to bytes, as a real crash yields only a subset.

use super::digest::{Digest, sha256};
use super::format::*;

fn image(first_seq: u64, payloads: &[&[u8]]) -> Vec<u8> {
    let base = Digest::genesis();
    let mut out = segment_header(first_seq, base).to_vec();
    let mut prev = base;
    for (i, p) in payloads.iter().enumerate() {
        let seq = first_seq + i as u64;
        let link = entry_link(prev, seq, p);
        out.extend_from_slice(&entry_header(seq, p.len() as u32, link));
        out.extend_from_slice(p);
        prev = link;
    }
    out
}

#[test]
fn a_well_formed_segment_is_clean() {
    let img = image(1, &[b"alpha", b"beta", b"gamma"]);
    match verify_image(&img) {
        Verdict::Clean {
            entries, first_seq, last_seq, incomplete_tail_bytes, sealed_len, ..
        } => {
            assert_eq!(entries, 3);
            assert_eq!(first_seq, 1);
            assert_eq!(last_seq, 3);
            assert_eq!(incomplete_tail_bytes, 0);
            assert_eq!(sealed_len as usize, img.len());
        }
        other => panic!("expected Clean, got {other:?}"),
    }
}

/// An empty segment is Clean with zero entries - not Unreadable.
/// The event model: an unreadable store must never be indistinguishable from an empty one.
#[test]
fn an_empty_segment_is_clean_not_unreadable() {
    let img = image(1, &[]);
    match verify_image(&img) {
        Verdict::Clean { entries, .. } => assert_eq!(entries, 0),
        other => panic!("expected Clean, got {other:?}"),
    }
}

#[test]
fn nothing_at_all_is_unreadable_not_empty() {
    assert_eq!(
        verify_image(&[]),
        Verdict::Unreadable { reason: UnreadableReason::TruncatedHeader }
    );
}

#[test]
fn a_foreign_file_is_unreadable() {
    let mut img = image(1, &[b"x"]);
    img[0] = b'X';
    assert_eq!(verify_image(&img), Verdict::Unreadable { reason: UnreadableReason::BadMagic });
}

/// A future format version is refused, not parsed optimistically.
#[test]
fn a_newer_format_version_is_refused() {
    let mut img = image(1, &[b"x"]);
    img[4..6].copy_from_slice(&(FORMAT_VERSION + 1).to_be_bytes());
    assert_eq!(
        verify_image(&img),
        Verdict::Unreadable {
            reason: UnreadableReason::UnknownFormatVersion { found: FORMAT_VERSION + 1 }
        }
    );
}

/// Killed mid-append: the payload never landed.
/// Not Corrupt: those bytes were never a record, since `append` returns only once durable.
#[test]
fn a_torn_tail_is_clean_with_the_tail_reported() {
    let mut img = image(1, &[b"alpha", b"beta"]);
    let good_len = img.len();
    img.extend_from_slice(&entry_header(3, 500, Digest::genesis()));
    img.extend_from_slice(b"only a few of the five hundred bytes");

    match verify_image(&img) {
        Verdict::Clean { entries, last_seq, sealed_len, incomplete_tail_bytes, .. } => {
            assert_eq!(entries, 2);
            assert_eq!(last_seq, 2);
            assert_eq!(sealed_len as usize, good_len, "seal at the last whole entry");
            assert_eq!(incomplete_tail_bytes, (img.len() - good_len) as u64);
        }
        other => panic!("expected Clean with a tail, got {other:?}"),
    }
}

/// Killed between writing the header and any of the payload - still a torn tail.
#[test]
fn a_tail_shorter_than_a_header_is_still_clean() {
    let mut img = image(1, &[b"alpha"]);
    let good_len = img.len();
    img.extend_from_slice(b"EN\x00\x00");

    match verify_image(&img) {
        Verdict::Clean { entries, sealed_len, incomplete_tail_bytes, .. } => {
            assert_eq!(entries, 1);
            assert_eq!(sealed_len as usize, good_len);
            assert_eq!(incomplete_tail_bytes, 4);
        }
        other => panic!("expected Clean, got {other:?}"),
    }
}

/// A flipped byte inside a payload breaks the link. Corrupt, at the entry that failed.
#[test]
fn a_flipped_payload_byte_is_corrupt() {
    let img = image(1, &[b"alpha", b"beta"]);
    let mut damaged = img.clone();
    let payload_start = SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN;
    damaged[payload_start] ^= 0x01;

    match verify_image(&damaged) {
        Verdict::Corrupt { reason, at_offset } => {
            assert_eq!(reason, CorruptReason::LinkMismatch);
            assert_eq!(at_offset, SEGMENT_HEADER_LEN as u64);
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

/// Rewriting an entry and its link still fails: the chain reaches forward.
/// Every later link was computed over the old one, so editing and re-linking one entry is not enough.
#[test]
fn rewriting_an_entry_with_a_consistent_link_still_breaks_the_chain() {
    let img = image(1, &[b"alpha", b"beta", b"gamma"]);
    let mut damaged = img.clone();

    // Replace entry 1's payload with a same-length forgery and fix up its own link.
    let forged: &[u8] = b"ALPHA";
    let link = entry_link(Digest::genesis(), 1, forged);
    damaged[SEGMENT_HEADER_LEN..SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN]
        .copy_from_slice(&entry_header(1, forged.len() as u32, link));
    let ps = SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN;
    damaged[ps..ps + forged.len()].copy_from_slice(forged);

    match verify_image(&damaged) {
        Verdict::Corrupt { reason, at_offset } => {
            assert_eq!(reason, CorruptReason::LinkMismatch);
            // Entry 1 now verifies against itself. Entry 2 is where the forgery shows.
            assert_eq!(at_offset, (SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN + 5) as u64);
        }
        other => panic!("expected Corrupt at the following entry, got {other:?}"),
    }
}

/// A removed entry shows as a sequence break, not as a shorter clean segment.
#[test]
fn a_dropped_entry_is_a_sequence_break() {
    let base = Digest::genesis();
    let mut img = segment_header(1, base).to_vec();
    // Write seq 1, then jump to seq 3 with links that are each self-consistent.
    let mut prev = base;
    for seq in [1u64, 3u64] {
        let p: &[u8] = b"x";
        let link = entry_link(prev, seq, p);
        img.extend_from_slice(&entry_header(seq, 1, link));
        img.extend_from_slice(p);
        prev = link;
    }
    match verify_image(&img) {
        Verdict::Corrupt { reason, .. } => {
            assert_eq!(reason, CorruptReason::SequenceBreak { expected: 2, found: 3 })
        }
        other => panic!("expected a sequence break, got {other:?}"),
    }
}

/// A corrupted length field is refused before it is used.
/// Without the bound, four damaged bytes become an allocation failure unrelated to the corruption.
#[test]
fn an_absurd_payload_length_is_refused_before_allocating() {
    let mut img = image(1, &[b"alpha"]);
    img[SEGMENT_HEADER_LEN + 2..SEGMENT_HEADER_LEN + 6]
        .copy_from_slice(&0xffff_ffffu32.to_be_bytes());
    match verify_image(&img) {
        Verdict::Corrupt { reason, .. } => {
            assert_eq!(reason, CorruptReason::PayloadLengthOutOfRange { found: 0xffff_ffff })
        }
        other => panic!("expected a length refusal, got {other:?}"),
    }
}

/// Garbage where an entry should start is corruption, not a torn tail.
/// A torn tail is short, damage is wrong.
#[test]
fn a_bad_entry_marker_is_corrupt_not_a_tail() {
    let mut img = image(1, &[b"alpha", b"beta"]);
    let second = SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN + 5;
    img[second] = b'Z';
    match verify_image(&img) {
        Verdict::Corrupt { reason, at_offset } => {
            assert_eq!(reason, CorruptReason::EntryMarker);
            assert_eq!(at_offset, second as u64);
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

/// Entries are readable from a clean segment and refused from a condemned one.
/// Quarantine has to hold at the API: a condemned segment must not be readable.
#[test]
fn a_condemned_segment_yields_no_entries() {
    let img = image(7, &[b"alpha", b"beta"]);
    let got = read_entries(&img).expect("clean segment reads");
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].seq, 7);
    assert_eq!(got[1].payload, b"beta".to_vec());

    let mut damaged = img.clone();
    damaged[SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN] ^= 0xff;
    assert!(read_entries(&damaged).is_none(), "quarantine must refuse reads");
}

/// The same payload at a different position gets a different link.
/// Position is part of the record, or identical events could be reordered unnoticed.
#[test]
fn position_is_part_of_the_link() {
    let a = entry_link(Digest::genesis(), 1, b"same");
    let b = entry_link(Digest::genesis(), 2, b"same");
    assert_ne!(a, b);
}

/// An empty payload is a legitimate entry, not an absent one.
#[test]
fn an_empty_payload_is_a_real_entry() {
    let img = image(1, &[b"", b"after"]);
    match verify_image(&img) {
        Verdict::Clean { entries, .. } => assert_eq!(entries, 2),
        other => panic!("expected Clean, got {other:?}"),
    }
    let got = read_entries(&img).unwrap();
    assert_eq!(got[0].payload, Vec::<u8>::new());
    assert_ne!(got[0].link, sha256(b""), "an entry link is not a bare payload hash");
}

/// Overwrite the length field of the entry at `entry_offset` with `claimed`.
fn claim_length(img: &mut [u8], entry_offset: usize, claimed: u32) {
    img[entry_offset + 2..entry_offset + 6].copy_from_slice(&claimed.to_be_bytes());
}

/// A damaged length that points past the end looks like a torn tail by shape alone.
/// It is not one: the entry it describes is whole, and every entry after it was acknowledged.
#[test]
fn a_length_damaged_past_the_end_mid_segment_is_corrupt_not_a_torn_tail() {
    let mut img = image(1, &[b"alpha", b"beta", b"gamma"]);
    let second = SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN + b"alpha".len();
    claim_length(&mut img, second, 70_000);

    match verify_image(&img) {
        Verdict::Corrupt { reason, at_offset } => {
            assert_eq!(reason, CorruptReason::PayloadLengthDamaged { found: 70_000 });
            assert_eq!(at_offset, second as u64);
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

/// The last entry has no successor to betray it, so its own link over the bytes that remain decides.
#[test]
fn a_length_damaged_past_the_end_on_the_last_entry_is_corrupt() {
    let mut img = image(1, &[b"alpha", b"beta"]);
    let last = SEGMENT_HEADER_LEN + ENTRY_HEADER_LEN + b"alpha".len();
    claim_length(&mut img, last, 0x00FF_0000);

    match verify_image(&img) {
        Verdict::Corrupt { reason, at_offset } => {
            assert_eq!(reason, CorruptReason::PayloadLengthDamaged { found: 0x00FF_0000 });
            assert_eq!(at_offset, last as u64);
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

/// A torn batch: one whole header, part of its payload, then nothing. Still a torn tail.
#[test]
fn a_torn_tail_whose_payload_holds_a_marker_is_still_clean() {
    let mut img = image(1, &[b"alpha"]);
    let good_len = img.len();
    let mut torn = b"padding ".to_vec();
    torn.extend_from_slice(&entry_header(3, 4, Digest::genesis()));
    let link = entry_link(Digest::genesis(), 2, &[0u8; 400]);
    img.extend_from_slice(&entry_header(2, 400, link));
    img.extend_from_slice(&torn);

    match verify_image(&img) {
        Verdict::Clean { entries, sealed_len, .. } => {
            assert_eq!(entries, 1);
            assert_eq!(sealed_len as usize, good_len);
        }
        other => panic!("expected Clean, got {other:?}"),
    }
}
