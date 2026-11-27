// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Materialising what a run may read.
//! The event store is not mounted: the window describes what the run can reach, not what it is asked to respect.

use std::fs;
use std::path::Path;

use crate::runs::{Authority, RunSpec, Window};
use bijective_event_store::store::{StoreError, read_range};

#[derive(Debug)]
pub enum ExtractError {
    Store(StoreError),
    Write(std::io::Error),
    /// Refused, not served: an auditor class never sees market data.
    NotAMarketReader,
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Store(e) => write!(f, "store: {e:?}"),
            ExtractError::Write(e) => write!(f, "write: {e}"),
            ExtractError::NotAMarketReader => {
                write!(f, "this class reads the platform's record, not a market stream")
            }
        }
    }
}

impl std::error::Error for ExtractError {}

/// One file per entry: a concatenated file would let a run reading past its end find the next entry's bytes.
pub fn materialise<A: Authority>(
    spec: &RunSpec<A>,
    store_root: &Path,
    stream: &str,
    into: &Path,
) -> Result<usize, ExtractError> {
    let upper = match spec.window() {
        Window::ToPosition(p) => p,
        Window::ToPresent => u64::MAX,
        Window::RecordOnly => return Err(ExtractError::NotAMarketReader),
    };

    let entries = read_range(store_root, stream, 0, upper).map_err(ExtractError::Store)?;
    fs::create_dir_all(into).map_err(ExtractError::Write)?;

    for entry in &entries {
        // Zero-padded so directory order is sequence order whatever lists it.
        let name = format!("{:020}.event", entry.seq);
        fs::write(into.join(name), &entry.payload).map_err(ExtractError::Write)?;
    }

    Ok(entries.len())
}
