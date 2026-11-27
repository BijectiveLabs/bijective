// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Retention: removing a stream's oldest sealed segments, and recording where the stream now
//! begins so no read can mistake the shortened stream for a whole one.
//!
//! Only a prefix is removed, never a segment in the middle, and never the newest segment, which a
//! writer may hold open. The horizon is written before any segment is deleted: a crash part-way
//! leaves segments below the horizon, which reads ignore and the next prune removes.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::digest::{Digest, Sha256};
use crate::format::{Verdict, parse_segment_header};
use crate::store::{
    StoreError, manifest_path, read_file, read_manifest, segment_indices, segment_path, stream_dir,
};

const HORIZON: &str = "horizon";

/// Where a pruned stream begins: the first position it still holds and the chain link that
/// position extends, so the retained segments stay verifiable on their own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Horizon {
    pub stream: String,
    pub first_seq: u64,
    pub base_link: Digest,
}

impl Horizon {
    fn body(&self) -> String {
        format!(
            "stream {}\nfirst_seq {}\nbase_link {}\n",
            self.stream,
            self.first_seq,
            self.base_link.to_hex()
        )
    }

    fn render(&self) -> String {
        let body = self.body();
        format!("{body}horizon_digest {}\n", digest_of(&body).to_hex())
    }
}

/// What one prune removed. A prune that removes nothing is reported, not refused.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub segments: u64,
    pub entries: u64,
    pub bytes: u64,
    /// The stream's first held position after the prune. `None` if it was never pruned.
    pub horizon: Option<u64>,
}

fn digest_of(text: &str) -> Digest {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.finish()
}

/// The stream's horizon, `None` for a stream never pruned.
/// Error contract: a horizon that does not parse or match its digest is refused, because a reader
/// that ignored it would take a shortened stream for a whole one.
pub fn horizon(root: &Path, stream: &str) -> Result<Option<Horizon>, StoreError> {
    horizon_in(&stream_dir(root, stream)?)
}

pub(crate) fn horizon_in(dir: &Path) -> Result<Option<Horizon>, StoreError> {
    let path = dir.join(HORIZON);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    parse(&text).map(Some).ok_or(StoreError::HorizonUnreadable { path })
}

fn parse(text: &str) -> Option<Horizon> {
    let mut fields: BTreeMap<&str, &str> = BTreeMap::new();
    for line in text.lines() {
        let (k, v) = line.split_once(' ')?;
        fields.insert(k, v);
    }
    let horizon = Horizon {
        stream: (*fields.get("stream")?).to_string(),
        first_seq: fields.get("first_seq")?.parse().ok()?,
        base_link: Digest::from_hex(fields.get("base_link")?)?,
    };
    (digest_of(&horizon.body()) == Digest::from_hex(fields.get("horizon_digest")?)?)
        .then_some(horizon)
}

/// Removes every sealed segment whose last position is before `before_seq`, oldest first,
/// stopping at the first that is not, and never the newest segment.
/// Precondition: whatever the removed entries were kept for is held elsewhere. The caller proves
/// it. Postcondition: `read_range` refuses any position below the returned horizon.
pub fn prune_before(root: &Path, stream: &str, before_seq: u64) -> Result<Pruned, StoreError> {
    let dir = stream_dir(root, stream)?;
    let indices = segment_indices(&dir)?;
    let below = horizon_in(&dir)?.map_or(0, |h| h.first_seq);
    let unchanged = Pruned { horizon: below_or_none(below), ..Pruned::default() };
    let Some((&newest, older)) = indices.split_last() else {
        return Ok(unchanged);
    };

    let mut doomed = Vec::new();
    let mut keep_from = newest;
    for &index in older {
        match read_manifest(&manifest_path(&dir, index)) {
            Some(m) if m.entries == 0 || m.last_seq < before_seq => doomed.push((index, m)),
            _ => {
                keep_from = index;
                break;
            }
        }
    }
    if doomed.is_empty() {
        return Ok(unchanged);
    }

    let head = first_held(&dir, keep_from)?;
    let new_horizon = Horizon { stream: stream.to_string(), ..head };
    write_horizon(&dir, &new_horizon)?;

    // Counted by what was removed now: a segment an interrupted prune already took is not freed twice.
    let mut pruned = Pruned { horizon: Some(new_horizon.first_seq), ..Pruned::default() };
    for (index, m) in &doomed {
        if remove_if_present(&segment_path(&dir, *index))? {
            pruned.segments += 1;
            pruned.entries += m.entries;
            pruned.bytes += m.bytes;
        }
        remove_if_present(&manifest_path(&dir, *index))?;
    }
    Ok(pruned)
}

fn below_or_none(first_seq: u64) -> Option<u64> {
    (first_seq > 0).then_some(first_seq)
}

/// The first position and base link of the segment that becomes the stream's start.
fn first_held(dir: &Path, index: u64) -> Result<Horizon, StoreError> {
    let path = segment_path(dir, index);
    let bytes = read_file(&path)?;
    let head = parse_segment_header(&bytes).map_err(|reason| StoreError::Quarantined {
        segment: path.clone(),
        verdict: Verdict::Unreadable { reason },
    })?;
    Ok(Horizon { stream: String::new(), first_seq: head.first_seq, base_link: head.base_link })
}

/// Temporary name, sync, rename, sync the directory: the horizon is whole or absent.
fn write_horizon(dir: &Path, horizon: &Horizon) -> Result<(), StoreError> {
    let tmp: PathBuf = dir.join(format!("{HORIZON}.tmp"));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(horizon.render().as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, dir.join(HORIZON))?;
    File::open(dir)?.sync_all()?;
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<bool, StoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
#[path = "../tests/unit/retention_tests.rs"]
mod tests;
