// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Reading on from where a reader stopped, so a reader that polls verifies each byte once.
//!
//! `read_range` reads and verifies every segment whole on every call, so asking for the newest
//! ten entries of a long stream costs as much as reading all of it. A `Cursor` remembers the
//! segment, byte offset and chain link a read ended at. The next read opens that segment at the
//! offset and verifies only what follows, chained from that link.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::format::{
    Continuation, Entry, SEGMENT_HEADER_LEN, Verdict, entries_from, parse_segment_header,
};
use crate::store::{StoreError, read_file, segment_indices, segment_path, stream_dir};

/// Where a read of one stream ended. Made only by `read_after`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    segment: u64,
    at: Continuation,
}

impl Cursor {
    /// The position the next entry will have.
    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.at.next_seq
    }
}

/// The entries appended after `from`, or the whole stream from its horizon for `None`, oldest
/// first, and the cursor to read on from (`None` while the stream has no segment).
///
/// Bytes before the cursor are not read again: they were verified when it was made, and no code
/// path rewrites a byte already written. A segment shorter than the cursor's offset was replaced,
/// not appended to, and is an error, as is a failed segment: a read with a hole looks complete.
pub fn read_after(
    root: &Path,
    stream: &str,
    from: Option<Cursor>,
) -> Result<(Vec<Entry>, Option<Cursor>), StoreError> {
    let dir = stream_dir(root, stream)?;
    // A segment an interrupted prune left lies below the horizon. Its entries are not the stream's.
    let start = crate::retention::horizon_in(&dir)?.map_or(0, |h| h.first_seq);
    let mut out = Vec::new();
    let mut cursor = from;
    for index in segment_indices(&dir)? {
        if cursor.is_some_and(|c| index < c.segment) {
            continue;
        }
        let path = segment_path(&dir, index);
        let read = match cursor {
            Some(c) if c.segment == index => continue_segment(&path, c.at)?,
            _ => whole_segment(&read_file(&path)?),
        };
        let (mut entries, at) =
            read.map_err(|verdict| StoreError::Quarantined { segment: path, verdict })?;
        entries.retain(|e| e.seq >= start);
        out.extend(entries);
        cursor = Some(Cursor { segment: index, at });
    }
    Ok((out, cursor))
}

/// A segment's entries and where they end, or the verdict that condemns it.
type SegmentRead = Result<(Vec<Entry>, Continuation), Verdict>;

fn whole_segment(bytes: &[u8]) -> SegmentRead {
    let head = parse_segment_header(bytes).map_err(|reason| Verdict::Unreadable { reason })?;
    let from = Continuation {
        offset: SEGMENT_HEADER_LEN as u64,
        next_seq: head.first_seq,
        prior_link: head.base_link,
    };
    entries_from(&bytes[SEGMENT_HEADER_LEN..], from)
}

fn continue_segment(path: &Path, at: Continuation) -> Result<SegmentRead, StoreError> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() < at.offset {
        return Err(StoreError::Rewritten { segment: path.to_path_buf() });
    }
    file.seek(SeekFrom::Start(at.offset))?;
    let mut appended = Vec::new();
    file.read_to_end(&mut appended)?;
    Ok(entries_from(&appended, at))
}
