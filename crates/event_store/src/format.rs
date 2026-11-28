// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The on-disk format, and the reader that judges it.
//! Pure functions over bytes. Nothing here opens a file. The verdicts are explained in `docs/concepts/event_sourcing.md`.

use crate::digest::{Digest, Sha256};

pub const SEGMENT_MAGIC: [u8; 4] = *b"BJES";

/// On-disk format version. A segment from a future version is refused, never partly parsed.
pub const FORMAT_VERSION: u16 = 1;

/// magic(4) + version(2) + reserved(2) + first_seq(8) + base_link(32)
pub const SEGMENT_HEADER_LEN: usize = 48;

/// entry_magic(2) + payload_len(4) + seq(8) + link(32)
pub const ENTRY_HEADER_LEN: usize = 46;

const ENTRY_MAGIC: [u8; 2] = *b"EN";

/// Upper bound on one payload, checked before any allocation.
/// A length read from disk is a claim: checked before allocation so damage cannot become an allocation failure.
pub const MAX_PAYLOAD_LEN: u32 = 16 * 1024 * 1024;

/// What a segment turned out to be. The three-way split is the event model's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every complete entry chains correctly.
    /// `incomplete_tail_bytes` is the crash signature, reported not hidden. Never overwritten.
    Clean {
        entries: u64,
        first_seq: u64,
        last_seq: u64,
        last_link: Digest,
        /// Offset one past the last complete entry. The segment's true end.
        sealed_len: u64,
        incomplete_tail_bytes: u64,
    },
    /// A complete entry does not match the chain. Quarantine. Never repair.
    Corrupt { at_offset: u64, reason: CorruptReason },
    /// The segment could not be read as a segment at all. Distinct from an empty one, which is `Clean`.
    Unreadable { reason: UnreadableReason },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorruptReason {
    /// The entry's stored link does not equal the link recomputed over its own bytes.
    LinkMismatch,
    /// The sequence position skipped or repeated. the event model: gap-free within a stream.
    SequenceBreak { expected: u64, found: u64 },
    /// The two-byte entry marker is wrong where a complete entry was expected.
    EntryMarker,
    /// A length field exceeds `MAX_PAYLOAD_LEN`.
    PayloadLengthOutOfRange { found: u32 },
    /// A length field runs past the end, yet the entry it heads is whole: the length was damaged.
    PayloadLengthDamaged { found: u32 },
    /// A sealed segment does not hold what its manifest says was sealed: cut short, or re-cut.
    ManifestMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnreadableReason {
    TruncatedHeader,
    BadMagic,
    /// Written by a format version this build does not know.
    UnknownFormatVersion {
        found: u16,
    },
    IoError,
}

/// Chain link over the predecessor's link, position, length and payload.
/// Position and length are included so entries cannot be swapped or re-cut undetected.
pub fn entry_link(prev_link: Digest, seq: u64, payload: &[u8]) -> Digest {
    let mut h = Sha256::new();
    h.update(&prev_link.0);
    h.update(&seq.to_be_bytes());
    h.update(&(payload.len() as u32).to_be_bytes());
    h.update(payload);
    h.finish()
}

pub fn segment_header(first_seq: u64, base_link: Digest) -> [u8; SEGMENT_HEADER_LEN] {
    let mut out = [0u8; SEGMENT_HEADER_LEN];
    out[0..4].copy_from_slice(&SEGMENT_MAGIC);
    out[4..6].copy_from_slice(&FORMAT_VERSION.to_be_bytes());
    // 6..8 reserved, left zero.
    out[8..16].copy_from_slice(&first_seq.to_be_bytes());
    out[16..48].copy_from_slice(&base_link.0);
    out
}

pub fn entry_header(seq: u64, payload_len: u32, link: Digest) -> [u8; ENTRY_HEADER_LEN] {
    let mut out = [0u8; ENTRY_HEADER_LEN];
    out[0..2].copy_from_slice(&ENTRY_MAGIC);
    out[2..6].copy_from_slice(&payload_len.to_be_bytes());
    out[6..14].copy_from_slice(&seq.to_be_bytes());
    out[14..46].copy_from_slice(&link.0);
    out
}

pub struct SegmentHead {
    pub first_seq: u64,
    pub base_link: Digest,
}

pub fn parse_segment_header(bytes: &[u8]) -> Result<SegmentHead, UnreadableReason> {
    if bytes.len() < SEGMENT_HEADER_LEN {
        return Err(UnreadableReason::TruncatedHeader);
    }
    if bytes[0..4] != SEGMENT_MAGIC {
        return Err(UnreadableReason::BadMagic);
    }
    let version = u16::from_be_bytes([bytes[4], bytes[5]]);
    if version != FORMAT_VERSION {
        return Err(UnreadableReason::UnknownFormatVersion { found: version });
    }
    let mut seq = [0u8; 8];
    seq.copy_from_slice(&bytes[8..16]);
    let mut link = [0u8; 32];
    link.copy_from_slice(&bytes[16..48]);
    Ok(SegmentHead { first_seq: u64::from_be_bytes(seq), base_link: Digest(link) })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub seq: u64,
    pub payload: Vec<u8>,
    pub link: Digest,
}

/// Walk a whole segment image and judge it.
/// Bytes, not a file, so fault injection can supply damage a real crash would take hours to produce.
pub fn verify_image(bytes: &[u8]) -> Verdict {
    let head = match parse_segment_header(bytes) {
        Ok(h) => h,
        Err(reason) => return Verdict::Unreadable { reason },
    };
    let from = Continuation {
        offset: SEGMENT_HEADER_LEN as u64,
        next_seq: head.first_seq,
        prior_link: head.base_link,
    };
    match walk(&bytes[SEGMENT_HEADER_LEN..], from, None) {
        Ok(w) => Verdict::Clean {
            entries: w.entries,
            first_seq: head.first_seq,
            last_seq: w.at.next_seq.wrapping_sub(1),
            last_link: w.at.prior_link,
            sealed_len: w.at.offset,
            incomplete_tail_bytes: w.tail,
        },
        Err(corrupt) => corrupt,
    }
}

/// A point inside a segment that a walk can continue from: the byte offset one past the last
/// complete entry, the position expected next, and the link that entry carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Continuation {
    pub offset: u64,
    pub next_seq: u64,
    pub prior_link: Digest,
}

struct Walked {
    entries: u64,
    at: Continuation,
    tail: u64,
}

/// The complete entries of `bytes`, which are a segment's bytes from `from.offset` on, chained
/// from `from.prior_link`. Postcondition: `Ok` holds every complete entry in order and where to
/// continue. A torn tail is left for the next read. `Err` is the `Corrupt` verdict, at its
/// offset in the segment, and nothing before it is returned.
pub fn entries_from(
    bytes: &[u8],
    from: Continuation,
) -> Result<(Vec<Entry>, Continuation), Verdict> {
    let mut out = Vec::new();
    let walked = walk(bytes, from, Some(&mut out))?;
    Ok((out, walked.at))
}

/// One pass that judges and, when asked, collects, so no reader verifies a segment twice.
fn walk(
    bytes: &[u8],
    from: Continuation,
    mut out: Option<&mut Vec<Entry>>,
) -> Result<Walked, Verdict> {
    let mut offset = 0;
    let mut expected_seq = from.next_seq;
    let mut prev_link = from.prior_link;
    let mut entries: u64 = 0;
    let corrupt =
        |at: usize, reason| Verdict::Corrupt { at_offset: from.offset + at as u64, reason };

    loop {
        let remaining = bytes.len() - offset;
        let done = |entries, tail: usize| Walked {
            entries,
            at: Continuation {
                offset: from.offset + offset as u64,
                next_seq: expected_seq,
                prior_link: prev_link,
            },
            tail: tail as u64,
        };

        // Not enough for a header: either a clean end (nothing left) or an interrupted write.
        if remaining < ENTRY_HEADER_LEN {
            return Ok(done(entries, remaining));
        }

        let hdr = &bytes[offset..offset + ENTRY_HEADER_LEN];
        if hdr[0..2] != ENTRY_MAGIC {
            // The marker is wrong. This is not a torn tail - a torn tail is short, not wrong.
            return Err(corrupt(offset, CorruptReason::EntryMarker));
        }

        let payload_len = u32::from_be_bytes([hdr[2], hdr[3], hdr[4], hdr[5]]);
        if payload_len > MAX_PAYLOAD_LEN {
            let reason = CorruptReason::PayloadLengthOutOfRange { found: payload_len };
            return Err(corrupt(offset, reason));
        }

        let total = ENTRY_HEADER_LEN + payload_len as usize;
        if remaining < total {
            let mut link_b = [0u8; 32];
            link_b.copy_from_slice(&hdr[14..46]);
            let stored = Digest(link_b);
            if is_whole_under_damaged_length(&bytes[offset..], prev_link, expected_seq, stored) {
                let reason = CorruptReason::PayloadLengthDamaged { found: payload_len };
                return Err(corrupt(offset, reason));
            }
            // Header whole, payload not: still a torn tail, never acknowledged to a caller.
            return Ok(done(entries, remaining));
        }

        let mut seq_b = [0u8; 8];
        seq_b.copy_from_slice(&hdr[6..14]);
        let seq = u64::from_be_bytes(seq_b);
        if seq != expected_seq {
            let reason = CorruptReason::SequenceBreak { expected: expected_seq, found: seq };
            return Err(corrupt(offset, reason));
        }

        let mut link_b = [0u8; 32];
        link_b.copy_from_slice(&hdr[14..46]);
        let stored = Digest(link_b);

        let payload = &bytes[offset + ENTRY_HEADER_LEN..offset + total];
        if entry_link(prev_link, seq, payload) != stored {
            return Err(corrupt(offset, CorruptReason::LinkMismatch));
        }
        if let Some(out) = out.as_deref_mut() {
            out.push(Entry { seq, payload: payload.to_vec(), link: stored });
        }

        prev_link = stored;
        expected_seq = expected_seq.wrapping_add(1);
        entries += 1;
        offset += total;
    }
}

/// Whether an entry whose length runs past the end is in fact whole, so its length was damaged.
/// Shape alone cannot tell a damaged length from a torn write: both run past the end. A torn write
/// leaves a prefix of its payload, so the entry's own link verifies at no end. The ends tried are
/// the end of the bytes and each place a successor header could begin (marker and next position).
/// Cost: one scan of the tail plus one hash per candidate. Candidates are rare by construction, and
/// this runs only on a tail, so the order path never pays it.
fn is_whole_under_damaged_length(
    entry: &[u8],
    prev_link: Digest,
    seq: u64,
    stored: Digest,
) -> bool {
    let payload = &entry[ENTRY_HEADER_LEN..];
    if entry_link(prev_link, seq, payload) == stored {
        return true;
    }
    let next = seq.wrapping_add(1).to_be_bytes();
    let starts = payload.len().saturating_sub(ENTRY_HEADER_LEN - 1);
    (0..starts).any(|at| {
        let successor = &payload[at..at + ENTRY_HEADER_LEN];
        successor[0..2] == ENTRY_MAGIC
            && successor[6..14] == next
            && entry_link(prev_link, seq, &payload[..at]) == stored
    })
}

/// Read every entry up to the last complete one. `None` unless `Clean`, so a condemned segment cannot be read.
pub fn read_entries(bytes: &[u8]) -> Option<Vec<Entry>> {
    let head = parse_segment_header(bytes).ok()?;
    let from = Continuation {
        offset: SEGMENT_HEADER_LEN as u64,
        next_seq: head.first_seq,
        prior_link: head.base_link,
    };
    entries_from(&bytes[SEGMENT_HEADER_LEN..], from).ok().map(|(entries, _)| entries)
}

impl core::fmt::Display for Verdict {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Verdict::Clean { entries, first_seq, last_seq, .. } => {
                write!(f, "clean, {entries} entries from {first_seq} to {last_seq}")
            }
            Verdict::Corrupt { at_offset, reason } => {
                write!(f, "corrupt at byte {at_offset}: {reason:?}")
            }
            Verdict::Unreadable { reason } => write!(f, "unreadable: {reason:?}"),
        }
    }
}
