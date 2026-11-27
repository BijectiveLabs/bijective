// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The operational host's work, separated from the process that runs it.
//! The deployment kit: reports the verifier's verdict and never interprets it.

use std::path::Path;

use bijective_event_store::format::Verdict;
use bijective_event_store::store::verify_segment;

/// What one sweep of a store found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sweep {
    pub clean: usize,
    pub damaged: usize,
    /// Segments that verify clean yet end mid-entry: the crash signature, or a copy cut short.
    pub with_torn_tail: usize,
    pub torn_tail_bytes: u64,
}

impl Sweep {
    pub fn total(&self) -> usize {
        self.clean + self.damaged
    }

    /// An empty store is healthy: a fresh node has appended nothing, and flagging it would teach operators to ignore the report.
    pub fn is_healthy(&self) -> bool {
        self.damaged == 0
    }

    /// Clean and ending where the writer stopped. A live node with a torn tail is healthy. A restored copy with one lost bytes.
    pub fn is_complete(&self) -> bool {
        self.is_healthy() && self.with_torn_tail == 0
    }
}

/// Walk every segment under the root. A root with no segments is clean, not damaged.
pub fn sweep(root: &Path) -> Sweep {
    let mut found = Sweep::default();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "seg") {
                match verify_segment(&path) {
                    Verdict::Clean { incomplete_tail_bytes, .. } => {
                        found.clean += 1;
                        if incomplete_tail_bytes > 0 {
                            found.with_torn_tail += 1;
                            found.torn_tail_bytes += incomplete_tail_bytes;
                        }
                    }
                    _ => found.damaged += 1,
                }
            }
        }
    }
    found
}

#[cfg(test)]
#[path = "../../tests/unit/host/tests.rs"]
mod tests;
