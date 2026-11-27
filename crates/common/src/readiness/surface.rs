// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Writing and reading the document, without a reader ever seeing half of one.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::readiness::document::{DOCUMENT_VERSION, Health, MAX_AGE_NS, Readiness, ReadinessError};

/// World-readable on purpose: an operator and a gate both read it without privilege. It carries no secret.
#[cfg(unix)]
const DOCUMENT_MODE: u32 = 0o644;

pub struct Surface {
    path: PathBuf,
    program: String,
}

impl Surface {
    pub fn new(path: impl Into<PathBuf>, program: impl Into<String>) -> Self {
        Self { path: path.into(), program: program.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Replace the document atomically.
    /// Written to a sibling and renamed, so a reader never sees a partial write.
    pub fn report(
        &self,
        health: Health,
        detail: impl Into<String>,
        at_ns: i128,
    ) -> std::io::Result<()> {
        // A directory here is the usual mistake. Fail clearly rather than on the rename.
        if self.path.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::IsADirectory,
                format!(
                    "{} is a directory; the readiness surface is a single document",
                    self.path.display()
                ),
            ));
        }

        let doc = Readiness::new(self.program.clone(), health, detail, at_ns);
        let body = serde_json::to_vec_pretty(&doc)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let parent = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;

        // Named after the document, not the program: one Data Engine runs per venue and instances share a run directory.
        let name = self
            .path
            .file_name()
            .map_or_else(|| self.program.clone(), |n| n.to_string_lossy().into_owned());
        let temp = parent.join(format!(".{name}.readiness.tmp"));
        {
            let mut f = std::fs::File::create(&temp)?;
            #[cfg(unix)]
            f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(DOCUMENT_MODE))?;
            f.write_all(&body)?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        std::fs::rename(&temp, &self.path)
    }

    /// Report, and complain to stderr if it could not be written.
    /// For reports after the first: swallowing the error would make a silent node look healthy. It does not stop the program.
    pub fn report_or_warn(&self, health: Health, detail: impl Into<String>, at_ns: i128) {
        if let Err(e) = self.report(health, detail, at_ns) {
            eprintln!(
                "{}: the readiness surface at {} could not be written: {e}",
                self.program,
                self.path.display()
            );
        }
    }
}

/// Read a program's claim, and judge it against the reader's own clock.
/// Staleness is decided here, not trusted from the document: a stopped program cannot update the field.
pub fn read_at(path: &Path, now_ns: i128) -> Result<Readiness, ReadinessError> {
    let raw = match std::fs::read(path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ReadinessError::Absent),
        Err(_) => return Err(ReadinessError::Unreadable),
    };

    let doc: Readiness = serde_json::from_slice(&raw).map_err(|_| ReadinessError::Unreadable)?;
    if doc.version != DOCUMENT_VERSION {
        return Err(ReadinessError::VersionUnknown { found: doc.version });
    }

    let stamped = doc.stamped_at_ns().ok_or(ReadinessError::Unreadable)?;
    let age = now_ns - stamped;
    if age > MAX_AGE_NS {
        return Err(ReadinessError::Stale { age_ns: age });
    }

    Ok(doc)
}

/// Is this program ready, with no way for an error to read as a yes.
/// Every failure to establish readiness returns false. An unverified node must never look healthy.
pub fn is_ready(path: &Path, now_ns: i128) -> bool {
    read_at(path, now_ns).map(|d| d.health.is_ready()).unwrap_or(false)
}
