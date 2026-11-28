// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The file-backed store: append, seal, recover, read.
//! Nothing here rewrites a byte already written. `append` is on the order path and does the least that is durable (`docs/concepts/event_sourcing.md`).

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::digest::{Digest, Sha256};
use crate::format::*;

/// Default segment size before rotation. Bounds what one corrupt segment quarantines.
pub const DEFAULT_MAX_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum StoreError {
    /// Another writer holds this stream. Single writer per stream.
    StreamLocked {
        stream: String,
    },
    /// A segment failed verification. The store refuses to open rather than skip it.
    Quarantined {
        segment: PathBuf,
        verdict: Verdict,
    },
    PayloadTooLarge {
        len: usize,
    },
    BadStreamName,
    /// A read asked for positions a prune removed. The stream begins at `horizon`.
    Pruned {
        stream: String,
        horizon: u64,
    },
    /// The horizon file does not parse or match its digest, so where the stream begins is unknown.
    HorizonUnreadable {
        path: PathBuf,
    },
    /// A segment is shorter than a reader's cursor into it: it was replaced, not appended to.
    Rewritten {
        segment: PathBuf,
    },
    Io(std::io::Error),
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::StreamLocked { stream } => {
                write!(f, "stream {stream} is already held by a writer")
            }
            StoreError::Quarantined { segment, verdict } => {
                write!(f, "segment {} is quarantined: {verdict:?}", segment.display())
            }
            StoreError::PayloadTooLarge { len } => write!(f, "payload of {len} bytes is too large"),
            StoreError::BadStreamName => write!(f, "stream name is not usable as a directory"),
            StoreError::Pruned { stream, horizon } => {
                write!(f, "stream {stream} was pruned and begins at position {horizon}")
            }
            StoreError::HorizonUnreadable { path } => {
                write!(f, "horizon {} is unreadable or edited", path.display())
            }
            StoreError::Rewritten { segment } => {
                write!(f, "segment {} is shorter than when it was read", segment.display())
            }
            StoreError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Resolve a stream directory, refusing any name that could escape the root.
/// An allow-list, not a deny-list: a deny-list is a list someone forgets to extend.
pub(crate) fn stream_dir(root: &Path, stream: &str) -> Result<PathBuf, StoreError> {
    if stream.is_empty() || stream.len() > 128 {
        return Err(StoreError::BadStreamName);
    }
    if !stream.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(StoreError::BadStreamName);
    }
    Ok(root.join(stream))
}

pub(crate) fn segment_path(dir: &Path, index: u64) -> PathBuf {
    dir.join(format!("{index:012}.seg"))
}

pub(crate) fn manifest_path(dir: &Path, index: u64) -> PathBuf {
    dir.join(format!("{index:012}.manifest"))
}

pub(crate) fn segment_indices(dir: &Path) -> Result<Vec<u64>, StoreError> {
    let mut found = Vec::new();
    if !dir.exists() {
        return Ok(found);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(stem) = name.strip_suffix(".seg")
            && let Ok(index) = stem.parse::<u64>()
        {
            found.push(index);
        }
    }
    found.sort_unstable();
    Ok(found)
}

pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>, StoreError> {
    let mut buf = Vec::new();
    File::open(path)?.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Judge one segment file. A file that cannot be opened is `Unreadable`, never empty.
pub fn verify_segment(path: &Path) -> Verdict {
    match read_file(path) {
        Ok(bytes) => judge(path, &bytes),
        Err(_) => Verdict::Unreadable { reason: UnreadableReason::IoError },
    }
}

/// Judge a segment's bytes, and hold a sealed one to its manifest.
/// A segment cut short at an entry boundary chains cleanly, so only the manifest can tell it was cut.
/// A manifest that is absent or does not verify leaves the judgement to the bytes: recovery seals
/// after a crash, and an interrupted seal must not make the stream unopenable.
fn judge(segment: &Path, bytes: &[u8]) -> Verdict {
    let verdict = verify_image(bytes);
    let Verdict::Clean { entries, first_seq, last_link, sealed_len, .. } = verdict else {
        return verdict;
    };
    match read_manifest(&segment.with_extension("manifest")) {
        Some(m)
            if m.entries != entries
                || m.first_seq != first_seq
                || m.bytes != sealed_len
                || m.last_link != last_link =>
        {
            Verdict::Corrupt { at_offset: sealed_len, reason: CorruptReason::ManifestMismatch }
        }
        _ => verdict,
    }
}

/// What a sealed segment declares about itself.
/// Written as text on purpose: audit reporting hands evidence to an operator's auditor who needs no tooling to read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub format: u16,
    pub stream: String,
    pub segment: u64,
    pub first_seq: u64,
    pub last_seq: u64,
    pub entries: u64,
    pub bytes: u64,
    pub base_link: Digest,
    pub last_link: Digest,
    /// Digest of the lines above, so a manifest edited by hand does not pass unnoticed.
    pub manifest_digest: Digest,
}

impl Manifest {
    fn body(&self) -> String {
        format!(
            "format {}\nstream {}\nsegment {}\nfirst_seq {}\nlast_seq {}\nentries {}\nbytes {}\nbase_link {}\nlast_link {}\n",
            self.format,
            self.stream,
            self.segment,
            self.first_seq,
            self.last_seq,
            self.entries,
            self.bytes,
            self.base_link.to_hex(),
            self.last_link.to_hex(),
        )
    }

    fn render(&self) -> String {
        let body = self.body();
        let mut h = Sha256::new();
        h.update(body.as_bytes());
        format!("{body}manifest_digest {}\n", h.finish().to_hex())
    }
}

/// A single-writer handle on one stream.
/// A stale lock is never broken automatically: two entries at one position cannot be repaired later.
#[derive(Debug)]
pub struct StreamWriter {
    stream: String,
    dir: PathBuf,
    segment_index: u64,
    file: File,
    next_seq: u64,
    last_link: Digest,
    segment_first_seq: u64,
    segment_bytes: u64,
    max_segment_bytes: u64,
    lock: PathBuf,
}

impl StreamWriter {
    /// Take the writer lock on `stream`, recovering whatever is already there.
    /// Fails closed on a corrupt or unreadable segment: appending would extend an unverified link.
    pub fn open(root: &Path, stream: &str) -> Result<Self, StoreError> {
        let dir = stream_dir(root, stream)?;
        fs::create_dir_all(&dir)?;

        let lock = dir.join(".writer.lock");
        match File::create_new(&lock) {
            Ok(mut f) => {
                let _ = writeln!(f, "{}", std::process::id());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(StoreError::StreamLocked { stream: stream.to_string() });
            }
            Err(e) => return Err(e.into()),
        }

        // From here a failure must release the lock, or a failed open leaves the stream unopenable.
        let opened = Self::open_locked(dir.clone(), stream, lock.clone());
        if opened.is_err() {
            let _ = fs::remove_file(&lock);
        }
        opened
    }

    fn open_locked(dir: PathBuf, stream: &str, lock: PathBuf) -> Result<Self, StoreError> {
        let indices = segment_indices(&dir)?;

        let (segment_index, first_seq, next_seq, last_link, bytes, fresh) = match indices.last() {
            None => (0u64, 0u64, 0u64, Digest::genesis(), 0u64, true),
            Some(&last) => {
                let path = segment_path(&dir, last);
                match verify_segment(&path) {
                    Verdict::Clean {
                        entries,
                        first_seq,
                        last_seq,
                        last_link,
                        sealed_len,
                        incomplete_tail_bytes,
                    } => {
                        let next = if entries == 0 { first_seq } else { last_seq + 1 };
                        if incomplete_tail_bytes > 0 {
                            // A crash left bytes that were never a record: seal and move on, tail untouched.
                            seal_at(
                                &dir, stream, last, first_seq, last_seq, entries, sealed_len,
                                last_link,
                            )?;
                            (last + 1, next, next, last_link, 0, true)
                        } else if manifest_path(&dir, last).exists() {
                            // Already sealed by an earlier run. Never reopen a sealed segment.
                            (last + 1, next, next, last_link, 0, true)
                        } else {
                            (last, first_seq, next, last_link, sealed_len, false)
                        }
                    }
                    verdict => {
                        return Err(StoreError::Quarantined { segment: path, verdict });
                    }
                }
            }
        };

        let path = segment_path(&dir, segment_index);
        let file = if fresh {
            let mut f = OpenOptions::new().write(true).create_new(true).open(&path)?;
            f.write_all(&segment_header(first_seq, last_link))?;
            f.sync_data()?;
            f
        } else {
            OpenOptions::new().append(true).open(&path)?
        };

        Ok(StreamWriter {
            stream: stream.to_string(),
            dir,
            segment_index,
            file,
            next_seq,
            last_link,
            segment_first_seq: first_seq,
            segment_bytes: if fresh { SEGMENT_HEADER_LEN as u64 } else { bytes },
            max_segment_bytes: DEFAULT_MAX_SEGMENT_BYTES,
            lock,
        })
    }

    /// Override the rotation bound. Mainly for tests, which cannot afford 64 MiB per segment.
    pub fn with_max_segment_bytes(mut self, bytes: u64) -> Self {
        self.max_segment_bytes = bytes.max(SEGMENT_HEADER_LEN as u64 + 1);
        self
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_seq
    }

    /// Append `payload` and return the sequence position it was given.
    /// Returns only once durable: that ordering is record-before-act.
    pub fn append(&mut self, payload: &[u8]) -> Result<u64, StoreError> {
        let seq = self.append_unsynced(payload)?;
        self.sync()?;
        Ok(seq)
    }

    /// Append `payload` without waiting for the disk, for a writer that makes a batch durable at
    /// once with `sync`. A reader sees the entry as soon as it is written, so whatever is derived
    /// from it is written only after `sync` returns: that keeps record-before-act for the batch.
    pub fn append_unsynced(&mut self, payload: &[u8]) -> Result<u64, StoreError> {
        if payload.len() > MAX_PAYLOAD_LEN as usize {
            return Err(StoreError::PayloadTooLarge { len: payload.len() });
        }

        let entry_len = (ENTRY_HEADER_LEN + payload.len()) as u64;
        if self.segment_bytes + entry_len > self.max_segment_bytes
            && self.segment_bytes > SEGMENT_HEADER_LEN as u64
        {
            self.rotate()?;
        }

        let seq = self.next_seq;
        let link = entry_link(self.last_link, seq, payload);

        // Two writes, not one joined buffer, to avoid copying every payload on the order path. A crash between them is a torn tail.
        self.file.write_all(&entry_header(seq, payload.len() as u32, link))?;
        self.file.write_all(payload)?;

        self.last_link = link;
        self.next_seq += 1;
        self.segment_bytes += entry_len;
        Ok(seq)
    }

    /// Every entry written so far is durable when this returns.
    pub fn sync(&mut self) -> Result<(), StoreError> {
        self.file.sync_data()?;
        Ok(())
    }

    pub fn rotate(&mut self) -> Result<Manifest, StoreError> {
        // A manifest describes entries on disk, so entries appended without a sync are made durable first.
        self.file.sync_data()?;
        let entries = self.next_seq - self.segment_first_seq;
        let manifest = seal_at(
            &self.dir,
            &self.stream,
            self.segment_index,
            self.segment_first_seq,
            self.next_seq.saturating_sub(1),
            entries,
            self.segment_bytes,
            self.last_link,
        )?;

        self.segment_index += 1;
        self.segment_first_seq = self.next_seq;
        let path = segment_path(&self.dir, self.segment_index);
        let mut f = OpenOptions::new().write(true).create_new(true).open(&path)?;
        f.write_all(&segment_header(self.segment_first_seq, self.last_link))?;
        f.sync_data()?;
        self.file = f;
        self.segment_bytes = SEGMENT_HEADER_LEN as u64;
        Ok(manifest)
    }
}

impl Drop for StreamWriter {
    fn drop(&mut self) {
        // The lock is released even on unwind, so a leftover lock always means a crash.
        let _ = fs::remove_file(&self.lock);
    }
}

/// Seal a segment: manifest first, then read-only, so a crash leaves a described segment.
#[allow(clippy::too_many_arguments)]
fn seal_at(
    dir: &Path,
    stream: &str,
    index: u64,
    first_seq: u64,
    last_seq: u64,
    entries: u64,
    bytes: u64,
    last_link: Digest,
) -> Result<Manifest, StoreError> {
    let seg = segment_path(dir, index);
    let base_link = match read_file(&seg) {
        Ok(b) => {
            parse_segment_header(&b).map(|h| h.base_link).unwrap_or_else(|_| Digest::genesis())
        }
        Err(_) => Digest::genesis(),
    };

    let manifest = Manifest {
        format: FORMAT_VERSION,
        stream: stream.to_string(),
        segment: index,
        first_seq,
        last_seq,
        entries,
        bytes,
        base_link,
        last_link,
        manifest_digest: Digest::genesis(),
    };

    let rendered = manifest.render();
    let mpath = manifest_path(dir, index);
    let mut mf = OpenOptions::new().write(true).create(true).truncate(true).open(&mpath)?;
    mf.write_all(rendered.as_bytes())?;
    mf.sync_data()?;
    drop(mf);

    // Immutable by construction: no code path reopens a sealed segment for writing,
    // and the permission bit makes a mistake elsewhere fail at the filesystem.
    if let Ok(meta) = fs::metadata(&seg) {
        let mut perms = meta.permissions();
        perms.set_readonly(true);
        let _ = fs::set_permissions(&seg, perms);
    }

    let mut h = Sha256::new();
    h.update(manifest.body().as_bytes());
    Ok(Manifest { manifest_digest: h.finish(), ..manifest })
}

/// Read a manifest back and check it against itself. `None` if missing, malformed, or its digest does not match.
pub fn read_manifest(path: &Path) -> Option<Manifest> {
    let text = fs::read_to_string(path).ok()?;
    let mut fields: BTreeMap<&str, &str> = BTreeMap::new();
    for line in text.lines() {
        let (k, v) = line.split_once(' ')?;
        fields.insert(k, v);
    }

    let manifest = Manifest {
        format: fields.get("format")?.parse().ok()?,
        stream: (*fields.get("stream")?).to_string(),
        segment: fields.get("segment")?.parse().ok()?,
        first_seq: fields.get("first_seq")?.parse().ok()?,
        last_seq: fields.get("last_seq")?.parse().ok()?,
        entries: fields.get("entries")?.parse().ok()?,
        bytes: fields.get("bytes")?.parse().ok()?,
        base_link: Digest::from_hex(fields.get("base_link")?)?,
        last_link: Digest::from_hex(fields.get("last_link")?)?,
        manifest_digest: Digest::from_hex(fields.get("manifest_digest")?)?,
    };

    let mut h = Sha256::new();
    h.update(manifest.body().as_bytes());
    if h.finish() != manifest.manifest_digest {
        return None;
    }
    Some(manifest)
}

/// Whether this process can write the store, asked by **trying it**.
/// Asked up front, so a read-only store is found before the program reports ready.
/// Permission bits do not answer it: the mount, ownership and init system decide.
pub fn writable(root: &Path) -> Result<(), StoreError> {
    fs::create_dir_all(root)?;
    let probe = root.join(format!(".writable-{}", std::process::id()));
    fs::write(&probe, b"")?;
    fs::remove_file(&probe)?;
    Ok(())
}

/// The last position a stream holds, or `None` for a stream with no entries.
/// Answered from the newest segment, not by replay. A failed segment is an error: a stream reported short reads as ended.
pub fn end_of(root: &Path, stream: &str) -> Result<Option<u64>, StoreError> {
    let dir = stream_dir(root, stream)?;

    for index in segment_indices(&dir)?.into_iter().rev() {
        let path = segment_path(&dir, index);
        let bytes = read_file(&path)?;
        match judge(&path, &bytes) {
            Verdict::Clean { last_seq, entries, .. } => {
                if entries > 0 {
                    return Ok(Some(last_seq));
                }
            }
            verdict => {
                return Err(StoreError::Quarantined { segment: path, verdict });
            }
        }
    }

    Ok(None)
}

/// Replay a stream from `from` up to and including `to`, by sequence position, never by time.
/// A failed segment stops the read: a replay with a hole looks complete.
pub fn read_range(root: &Path, stream: &str, from: u64, to: u64) -> Result<Vec<Entry>, StoreError> {
    let dir = stream_dir(root, stream)?;
    let mut out = Vec::new();
    if from > to {
        return Ok(out);
    }
    let start = crate::retention::horizon_in(&dir)?.map_or(0, |h| h.first_seq);
    if from < start {
        return Err(StoreError::Pruned { stream: stream.to_string(), horizon: start });
    }

    for index in segment_indices(&dir)? {
        let path = segment_path(&dir, index);
        let bytes = read_file(&path)?;
        match judge(&path, &bytes) {
            Verdict::Clean { first_seq, last_seq, entries, .. } => {
                // A segment an interrupted prune left lies below the horizon, so below `from`.
                if entries == 0 || last_seq < from || first_seq > to {
                    continue;
                }
            }
            verdict => {
                return Err(StoreError::Quarantined { segment: path, verdict });
            }
        }
        let Some(entries) = read_entries(&bytes) else {
            continue;
        };
        for e in entries {
            if e.seq >= from && e.seq <= to {
                out.push(e);
            }
        }
    }
    Ok(out)
}

/// The newest `count` entries of a stream, oldest first, read from the newest segments only.
/// A failed segment within what is read is an error: a short tail reads as a quiet stream.
pub fn read_tail(root: &Path, stream: &str, count: usize) -> Result<Vec<Entry>, StoreError> {
    let dir = stream_dir(root, stream)?;
    // A segment an interrupted prune left lies below the horizon. Its entries are not the stream's.
    let start = crate::retention::horizon_in(&dir)?.map_or(0, |h| h.first_seq);
    let mut newest_first: Vec<Vec<Entry>> = Vec::new();
    let mut held = 0;
    for index in segment_indices(&dir)?.into_iter().rev() {
        if held >= count {
            break;
        }
        let path = segment_path(&dir, index);
        let bytes = read_file(&path)?;
        let verdict = judge(&path, &bytes);
        if !matches!(verdict, Verdict::Clean { .. }) {
            return Err(StoreError::Quarantined { segment: path, verdict });
        }
        let mut entries = read_entries(&bytes).unwrap_or_default();
        entries.retain(|e| e.seq >= start);
        held += entries.len();
        newest_first.push(entries);
    }
    let mut out: Vec<Entry> = newest_first.into_iter().rev().flatten().collect();
    let surplus = out.len().saturating_sub(count);
    out.drain(..surplus);
    Ok(out)
}
