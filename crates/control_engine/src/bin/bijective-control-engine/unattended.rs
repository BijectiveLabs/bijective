// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A paper run or a backtest with no operator registry resumes itself (decision 0015): nobody
//! could sign a resume, and neither places capital. A live node never does: it refuses to start
//! without a registry.

use bijective_event_store::store::{StreamWriter, end_of};
use bijective_serialization::signal::{Command, Instruction};

use crate::node::Node;
use crate::{Fatal, PROGRAM};

/// Appends one resume, unless a command is already waiting to be carried out. The next pass
/// carries it out as it carries out any command.
pub(crate) fn resume_itself(node: &Node, commands: &str, at_ns: i128) -> Result<(), Fatal> {
    let read = end_of(node.store_root(), commands)
        .map_err(|e| Fatal::failure(format!("{commands} could not be read: {e:?}")))?;
    if read.is_some_and(|end| end >= node.commands_from()) {
        return Ok(());
    }
    let command = Command {
        id: format!("unattended-resume-{at_ns}"),
        actor: "unattended".into(),
        role: bijective_authority::Role::Operator,
        instruction: Instruction::Resume,
        at_ns,
    };
    let payload = bijective_serialization::signal::command::encode(&command)
        .map_err(|e| Fatal::failure(format!("the resume could not be encoded: {e:?}")))?;
    let mut writer = StreamWriter::open(node.store_root(), commands)
        .map_err(|e| Fatal::failure(format!("{commands} could not be opened: {e:?}")))?;
    writer
        .append(&payload)
        .map_err(|e| Fatal::failure(format!("the resume was not recorded: {e:?}")))?;
    eprintln!(
        "{PROGRAM}: no operator registry and no capital placed; it resumes itself by a recorded command"
    );
    Ok(())
}
