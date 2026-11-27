// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The layouts that cross between engines.
//! Shared by the Data, Strategy and Control Engines: a layout in either would put one engine's vocabulary in the other's dependency graph.

pub mod command;
pub mod facts;
pub mod feedback;
pub mod fees;
pub mod instruments;
pub mod market;
pub mod node;
pub mod proposal;

pub use command::{Command, Instruction, PositionAdjustment};
pub use market::BookUpdate;
pub use proposal::{VERSION, decode, encode};
