// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! An MCP server over stdio for research agents (decision 0020).
//!
//! Every tool is a platform program run as a child process inside one workspace directory, so the
//! server adds no logic of its own to a result and links no engine. It cannot reach a credential,
//! a venue or the order path, and the `Agent environment` gate keeps it that way.

pub mod protocol;
pub mod tools;

mod panels;
mod run;
mod workspace;

pub use tools::Context;
pub use workspace::Workspace;
