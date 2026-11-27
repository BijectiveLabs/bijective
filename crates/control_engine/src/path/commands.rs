// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Operator commands on the order path.
//!
//! With an operator registry, a command is carried out only when it is signed by the operator it
//! names, acts with the registry's role, and has not been carried out before. Every refusal is
//! recorded as an applied command with its reason, and the mark advances: skipping a command
//! silently reads as carried out.

use bijective_authority::decide;
use bijective_identity::{OperatorRegistry, SIGNATURE_LEN};
use bijective_limits::{SafeState, TradingStateControl};
use bijective_serialization::signal::command::encode;
use bijective_serialization::signal::{Command, Instruction};

use super::{ControlEvent, OrderPath, PathError};

impl OrderPath {
    /// Whether the latest command carried out was believed and permitted.
    #[must_use]
    pub fn last_carried_out(&self) -> bool {
        self.carried_out
    }

    pub fn set_operators(&mut self, registry: OperatorRegistry) {
        self.operators = Some(registry);
    }

    /// Carry out one operator command and record what came of it. With a registry, the signature
    /// is checked first and a command that fails is recorded as refused. Without one, the role the
    /// command claims stands.
    pub fn carry_out(
        &mut self,
        command: &Command,
        signature: Option<&[u8; SIGNATURE_LEN]>,
        position: u64,
        at_ns: i128,
    ) -> Result<SafeState, PathError> {
        let refusal = self.untrusted(command, signature);
        self.apply_command(command, refusal, position, at_ns)
    }

    /// Why this command is not to be believed, or `None`.
    fn untrusted(
        &self,
        command: &Command,
        signature: Option<&[u8; SIGNATURE_LEN]>,
    ) -> Option<String> {
        let registry = self.operators.as_ref()?;
        let Ok(message) = encode(command) else {
            return Some("the command could not be encoded".into());
        };
        if let Err(failure) =
            registry.authenticate(&command.actor, command.role, &message, signature)
        {
            return Some(format!("refused: {failure}"));
        }
        let seen = self.replayed.state().is_some_and(|r| r.progress().has_applied(&command.id));
        seen.then(|| format!("refused: command {} was already carried out", command.id))
    }

    /// Carry out one operator command and record what came of it. The state recorded is the one read
    /// back, not the one asked for. A refusal is recorded too, and the mark advances either way.
    fn apply_command(
        &mut self,
        command: &Command,
        refusal: Option<String>,
        position: u64,
        at_ns: i128,
    ) -> Result<SafeState, PathError> {
        let action = command.instruction.action();
        let decision = decide(command.role, action);

        self.carried_out = refusal.is_none() && decision.is_permitted();
        let detail = if let Some(reason) = refusal {
            reason
        } else if decision.is_permitted() {
            match &command.instruction {
                Instruction::Halt => {
                    self.trading.set(SafeState::Halted);
                    String::new()
                }
                Instruction::Reduce => {
                    self.trading.set(SafeState::Reducing);
                    String::new()
                }
                // An unknown order may have executed: resuming would trade on a position the platform does not
                // knows, so the barrier holds until reconciliation settles it.
                Instruction::Resume => match self.resume_barrier() {
                    None => {
                        self.trading.resume(SafeState::Active);
                        String::new()
                    }
                    Some(reason) => reason,
                },
                Instruction::AdjustPosition { venue, adjustment } => {
                    self.adjust_position(command, venue, adjustment, at_ns)?
                }
                // Reducing, so nothing adds exposure while the flatten orders, which the pass
                // places from the record, close what is held.
                Instruction::Flatten => {
                    self.trading.set(SafeState::Reducing);
                    String::new()
                }
                // Carried by the sweep, which reads the orders this command named from the record.
                Instruction::CancelOrders => String::new(),
                // Carried by the caller, which owns the venue set. Routing must live in one place.
                Instruction::WithdrawVenue { .. } | Instruction::AdmitVenue { .. } => String::new(),
                // Recorded, not ignored: an unrecorded command would read as carried out.
                other => {
                    format!("{} is not the Control Engine's to carry out", other.action().as_str())
                }
            }
        } else {
            decision.to_string()
        };

        let state = self.trading.current();
        self.record(&ControlEvent::CommandApplied {
            command_id: command.id.clone(),
            actor: command.actor.clone(),
            role: command.role,
            action,
            state,
            detail,
            from_position: position,
            at_ns,
        })?;
        Ok(state)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/commands_tests.rs"]
mod tests;
