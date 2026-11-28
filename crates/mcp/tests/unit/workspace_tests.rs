// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! An agent's paths stay inside its workspace, however they are written.

use std::path::PathBuf;

use super::Workspace;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-mcp-ws-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("store")).unwrap();
    dir
}

#[test]
fn a_relative_path_resolves_under_the_root() {
    let ws = Workspace::new(&scratch("relative")).unwrap();
    assert_eq!(ws.resolve("store").unwrap(), ws.root().join("store"));
}

/// A strategy the agent is about to write does not exist yet and is still accepted.
#[test]
fn a_path_not_yet_written_is_accepted_inside_the_root() {
    let ws = Workspace::new(&scratch("new")).unwrap();
    assert_eq!(ws.resolve("research/alpha.py").unwrap(), ws.root().join("research/alpha.py"));
}

#[test]
fn climbing_out_is_refused() {
    let ws = Workspace::new(&scratch("climb")).unwrap();
    assert!(ws.resolve("../outside").is_err());
    assert!(ws.resolve("store/../../outside").is_err());
}

#[test]
fn an_absolute_path_elsewhere_is_refused() {
    let ws = Workspace::new(&scratch("absolute")).unwrap();
    assert!(ws.resolve("/etc/bijective/config.json").is_err());
}

/// A link inside the workspace that points out of it is judged by where it points.
#[cfg(unix)]
#[test]
fn a_link_out_of_the_workspace_is_refused() {
    let root = scratch("link");
    std::os::unix::fs::symlink("/etc", root.join("escape")).unwrap();
    let ws = Workspace::new(&root).unwrap();
    assert!(ws.resolve("escape/passwd").is_err());
}
