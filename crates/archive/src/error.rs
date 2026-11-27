// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Every way the archive refuses, named so an operator can act without reading source.

use std::path::PathBuf;

#[derive(Debug)]
pub enum ArchiveError {
    /// A fact with no archive table. Refused rather than dropped: a table arrives with its
    /// producer, and until then the class is not archived, visibly.
    NoTable {
        class: &'static str,
    },
    /// A figure wider than the column that holds it.
    OutOfRange {
        field: &'static str,
        value: i128,
    },
    /// Decimal columns hold at most eighteen digits, so a wider scale cannot be represented.
    ScaleUnsupported {
        scale: u8,
    },
    /// Another writer or a compaction holds this venue's archive.
    Locked {
        path: PathBuf,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parquet {
        path: PathBuf,
        detail: String,
    },
    /// A manifest line that does not parse. The manifest is the archive's index, so it is fatal.
    Manifest {
        line: usize,
        detail: String,
    },
    /// A file read back does not hold what it should: wrong schema, hash or row count.
    Corrupt {
        path: PathBuf,
        detail: String,
    },
    /// Nothing to compact, or a compaction asked of a partition that is still being written.
    NothingToCompact {
        partition: String,
    },
}

impl ArchiveError {
    pub(crate) fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        Self::Io { path: path.to_path_buf(), source }
    }

    pub(crate) fn parquet(path: &std::path::Path, e: impl core::fmt::Display) -> Self {
        Self::Parquet { path: path.to_path_buf(), detail: e.to_string() }
    }

    pub(crate) fn corrupt(path: &std::path::Path, detail: impl Into<String>) -> Self {
        Self::Corrupt { path: path.to_path_buf(), detail: detail.into() }
    }
}

impl core::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoTable { class } => write!(f, "no archive table holds {class} records yet"),
            Self::OutOfRange { field, value } => {
                write!(f, "{field} {value} does not fit a 64-bit column")
            }
            Self::ScaleUnsupported { scale } => {
                write!(f, "scale {scale} exceeds the 18 digits a decimal column holds")
            }
            Self::Locked { path } => write!(f, "{} is held by another process", path.display()),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parquet { path, detail } => write!(f, "{}: parquet: {detail}", path.display()),
            Self::Manifest { line, detail } => write!(f, "manifest line {line}: {detail}"),
            Self::Corrupt { path, detail } => write!(f, "{}: {detail}", path.display()),
            Self::NothingToCompact { partition } => write!(f, "nothing to compact in {partition}"),
        }
    }
}

impl std::error::Error for ArchiveError {}
