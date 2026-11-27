// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The one directory an agent works in. Every path a tool receives is resolved here.

use std::path::{Component, Path, PathBuf};

/// Where an agent may read and write. Paths outside it are refused before any program runs.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// The root is canonical from here on, so a symbolic link cannot later point it elsewhere.
    ///
    /// # Errors
    /// The root does not exist or is not a directory.
    pub fn new(root: &Path) -> Result<Self, String> {
        let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
        if !root.is_dir() {
            return Err(format!("{} is not a directory", root.display()));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The path `given` names inside the workspace, relative to its root or absolute.
    /// A path that does not exist yet is judged by its nearest existing parent, and no part of
    /// what remains may climb out with `..`.
    ///
    /// # Errors
    /// The path leaves the workspace.
    pub fn resolve(&self, given: &str) -> Result<PathBuf, String> {
        let asked = Path::new(given);
        let joined = if asked.is_absolute() { asked.to_path_buf() } else { self.root.join(asked) };
        let (existing, rest) = split_existing(&joined);
        let mut resolved = existing.canonicalize().map_err(|e| format!("{given}: {e}"))?;
        for part in rest {
            match part {
                Component::Normal(name) => resolved.push(name),
                _ => return Err(format!("{given} climbs out of the workspace")),
            }
        }
        if resolved.starts_with(&self.root) {
            Ok(resolved)
        } else {
            Err(format!("{given} is outside the workspace {}", self.root.display()))
        }
    }
}

/// The longest prefix of `path` that exists, and the components after it.
fn split_existing(path: &Path) -> (PathBuf, Vec<Component<'_>>) {
    let mut existing = path;
    let mut rest = Vec::new();
    while !existing.exists() {
        let Some(parent) = existing.parent() else { break };
        if let Some(last) = existing.components().next_back() {
            rest.push(last);
        }
        existing = parent;
    }
    rest.reverse();
    (existing.to_path_buf(), rest)
}

#[cfg(test)]
#[path = "../tests/unit/workspace_tests.rs"]
mod tests;
