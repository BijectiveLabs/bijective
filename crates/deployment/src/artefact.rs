// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The release artefact is verified before it is unpacked.
//! A checksum checked after unpacking has already written the contents.

use std::collections::BTreeMap;
use std::path::Path;

use bijective_event_store::digest::sha256;
use serde::{Deserialize, Serialize};

pub const CHECKSUMS_VERSION: u32 = 1;

/// Path to lowercase hex SHA-256, sorted, so two releases of the same content compare equal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checksums {
    pub version: u32,
    pub platform_version: String,
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtefactError {
    ChecksumsUnreadable,
    VersionUnknown {
        found: u32,
    },
    NoFiles,
    Missing {
        path: String,
    },
    Unreadable {
        path: String,
    },
    Mismatch {
        path: String,
        expected: String,
        found: String,
    },
    /// Present in the artefact and not named by the manifest. Refused: checking only what is listed permits additions.
    Unlisted {
        path: String,
    },
}

impl core::fmt::Display for ArtefactError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ChecksumsUnreadable => write!(f, "the checksum manifest could not be read"),
            Self::VersionUnknown { found } => {
                write!(f, "checksum manifest version {found} is not version {CHECKSUMS_VERSION}")
            }
            Self::NoFiles => write!(f, "the checksum manifest lists no files"),
            Self::Missing { path } => write!(f, "{path} is listed and absent"),
            Self::Unreadable { path } => write!(f, "{path} could not be read"),
            Self::Mismatch { path, expected, found } => {
                write!(f, "{path} hashes to {found}, not {expected}")
            }
            Self::Unlisted { path } => {
                write!(f, "{path} is in the artefact and not in the manifest")
            }
        }
    }
}

impl std::error::Error for ArtefactError {}

/// Verify every listed file and refuse anything unlisted. Returns the number of files checked.
pub fn verify_artefact(root: &Path, checksums: &Checksums) -> Result<usize, ArtefactError> {
    if checksums.version != CHECKSUMS_VERSION {
        return Err(ArtefactError::VersionUnknown { found: checksums.version });
    }
    if checksums.files.is_empty() {
        return Err(ArtefactError::NoFiles);
    }

    for (relative, expected) in &checksums.files {
        let path = root.join(relative);
        if !path.exists() {
            return Err(ArtefactError::Missing { path: relative.clone() });
        }
        let bytes = std::fs::read(&path)
            .map_err(|_| ArtefactError::Unreadable { path: relative.clone() })?;
        let found = sha256(&bytes).to_hex();
        if &found != expected {
            return Err(ArtefactError::Mismatch {
                path: relative.clone(),
                expected: expected.clone(),
                found,
            });
        }
    }

    for present in walk(root) {
        if !checksums.files.contains_key(&present) && present != CHECKSUMS_FILE {
            return Err(ArtefactError::Unlisted { path: present });
        }
    }

    Ok(checksums.files.len())
}

pub const CHECKSUMS_FILE: &str = "CHECKSUMS.json";

/// Build a checksum manifest over a directory, to cut a release and never to verify one.
pub fn checksums_for(root: &Path, platform_version: &str) -> Result<Checksums, ArtefactError> {
    let mut files = BTreeMap::new();
    for relative in walk(root) {
        if relative == CHECKSUMS_FILE {
            continue;
        }
        let bytes = std::fs::read(root.join(&relative))
            .map_err(|_| ArtefactError::Unreadable { path: relative.clone() })?;
        files.insert(relative, sha256(&bytes).to_hex());
    }
    Ok(Checksums {
        version: CHECKSUMS_VERSION,
        platform_version: platform_version.to_string(),
        files,
    })
}

fn walk(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}
