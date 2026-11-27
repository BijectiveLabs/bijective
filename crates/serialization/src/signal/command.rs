// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a person asks the platform to do, at version 1.
//! The console: a command is an event before it acts, carries who asked, and its outcome is read back.

use crate::codec::{ClassId, DecodeError, Reader, Writer, writer::EncodeError};
use bijective_authority::{Action, Role};
use bijective_model::{Owner, Side};

/// Version 2 carries the subject a command is about. Version 3 appends a position adjustment.
/// Version 4 appends a flag and, when set, the operator's signature. Version 5 appends, after the
/// signature, the desk and strategy an adjustment is for.
/// A version-1 record is refused, not read as platform-wide: guessing the subject widens one venue's withdrawal to all.
pub const VERSION: u16 = 5;

pub const SIGNATURE_LEN: usize = 64;

const ACTION_HALT: u8 = 0;
const ACTION_REDUCE: u8 = 1;
const ACTION_RESUME: u8 = 2;
const ACTION_CHANGE_LIMIT: u8 = 3;
const ACTION_ACKNOWLEDGE: u8 = 4;
const ACTION_APPROVE: u8 = 5;
const ACTION_WITHDRAW_VENUE: u8 = 6;
const ACTION_ADMIT_VENUE: u8 = 7;
const ACTION_ADJUST_POSITION: u8 = 8;
const ACTION_CANCEL_ORDERS: u8 = 9;
const ACTION_FLATTEN: u8 = 10;

const SIDE_BUY: u8 = 0;
const SIDE_SELL: u8 = 1;

/// A position change no fill explains, as a risk officer states it. The price is what the
/// quantity is carried at. `quantity_scaled` is positive and `side` says which way it moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionAdjustment {
    pub instrument: String,
    pub side: Side,
    pub quantity_scaled: i128,
    pub price_scaled: i128,
    /// Why, in the risk officer's words. Required, because the record is read by an auditor.
    pub reason: String,
    /// The desk and strategy the moved position belongs to. `None` leaves it nobody's, so it
    /// counts against every desk and strategy.
    pub owner: Option<Owner>,
}

/// One operator instruction, with whatever it is about.
/// The subject lives inside the variant that needs one, so the two cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    Halt,
    Reduce,
    Resume,
    ChangeLimit,
    AcknowledgeAlarm,
    ApproveRecommendation,
    WithdrawVenue { venue: String },
    AdmitVenue { venue: String },
    AdjustPosition { venue: String, adjustment: PositionAdjustment },
    CancelOrders,
    Flatten,
}

impl Instruction {
    /// The kind of action, which is what authority decides on. Never the subject, to avoid a second permission model.
    #[must_use]
    pub const fn action(&self) -> Action {
        match self {
            Instruction::Halt => Action::Halt,
            Instruction::Reduce => Action::Reduce,
            Instruction::Resume => Action::Resume,
            Instruction::ChangeLimit => Action::ChangeLimit,
            Instruction::AcknowledgeAlarm => Action::AcknowledgeAlarm,
            Instruction::ApproveRecommendation => Action::ApproveRecommendation,
            Instruction::WithdrawVenue { .. } => Action::WithdrawVenue,
            Instruction::AdmitVenue { .. } => Action::AdmitVenue,
            Instruction::AdjustPosition { .. } => Action::AdjustPosition,
            Instruction::CancelOrders => Action::CancelOrders,
            Instruction::Flatten => Action::Flatten,
        }
    }

    /// The venue this is about, where it is about one.
    #[must_use]
    pub fn venue(&self) -> Option<&str> {
        match self {
            Instruction::WithdrawVenue { venue }
            | Instruction::AdmitVenue { venue }
            | Instruction::AdjustPosition { venue, .. } => Some(venue),
            _ => None,
        }
    }
}

/// One operator action, as it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The issuer's own identifier for it, so an outcome can be joined to what was asked.
    pub id: String,
    /// Who asked. A name, not an authentication: that is the surface's job. The console requires attribution.
    pub actor: String,
    pub role: Role,
    pub instruction: Instruction,
    pub at_ns: i128,
}

const fn action_code(action: Action) -> u8 {
    match action {
        Action::Halt => ACTION_HALT,
        Action::Reduce => ACTION_REDUCE,
        Action::Resume => ACTION_RESUME,
        Action::ChangeLimit => ACTION_CHANGE_LIMIT,
        Action::AcknowledgeAlarm => ACTION_ACKNOWLEDGE,
        Action::ApproveRecommendation => ACTION_APPROVE,
        Action::WithdrawVenue => ACTION_WITHDRAW_VENUE,
        Action::AdmitVenue => ACTION_ADMIT_VENUE,
        Action::AdjustPosition => ACTION_ADJUST_POSITION,
        Action::CancelOrders => ACTION_CANCEL_ORDERS,
        Action::Flatten => ACTION_FLATTEN,
    }
}

fn action_from(code: u8) -> Result<Action, DecodeError> {
    match code {
        ACTION_HALT => Ok(Action::Halt),
        ACTION_REDUCE => Ok(Action::Reduce),
        ACTION_RESUME => Ok(Action::Resume),
        ACTION_CHANGE_LIMIT => Ok(Action::ChangeLimit),
        ACTION_ACKNOWLEDGE => Ok(Action::AcknowledgeAlarm),
        ACTION_APPROVE => Ok(Action::ApproveRecommendation),
        ACTION_WITHDRAW_VENUE => Ok(Action::WithdrawVenue),
        ACTION_ADMIT_VENUE => Ok(Action::AdmitVenue),
        ACTION_ADJUST_POSITION => Ok(Action::AdjustPosition),
        ACTION_CANCEL_ORDERS => Ok(Action::CancelOrders),
        ACTION_FLATTEN => Ok(Action::Flatten),
        other => Err(DecodeError::InvalidValue { field: "action", found: other as u64 }),
    }
}

/// The unsigned encoding, which is also the message a signature covers: the verifier decodes a
/// command and encodes it again, so what was signed is rebuilt, never sliced from the wire.
pub fn encode(command: &Command) -> Result<Vec<u8>, EncodeError> {
    write(command, None)
}

pub fn encode_signed(
    command: &Command,
    signature: &[u8; SIGNATURE_LEN],
) -> Result<Vec<u8>, EncodeError> {
    write(command, Some(signature))
}

fn write(
    command: &Command,
    signature: Option<&[u8; SIGNATURE_LEN]>,
) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::new(ClassId::OperatorCommand, VERSION);
    w.str(&command.id)?;
    w.str(&command.actor)?;
    // The role travels as its name, so an auditor reading later does not need this build.
    w.str(command.role.as_str())?;
    w.u8(action_code(command.instruction.action()));
    w.str(command.instruction.venue().unwrap_or(""))?;
    w.i128(command.at_ns);
    if let Instruction::AdjustPosition { adjustment, .. } = &command.instruction {
        write_adjustment(&mut w, adjustment)?;
    }
    match signature {
        None => {
            w.u8(0);
        }
        Some(signature) => {
            w.u8(1);
            w.bytes(signature)?;
        }
    }
    if let Instruction::AdjustPosition { adjustment, .. } = &command.instruction {
        write_owner(&mut w, adjustment.owner.as_ref())?;
    }
    Ok(w.finish())
}

/// A flag, then the desk, then a flag and the strategy.
fn write_owner(w: &mut Writer, owner: Option<&Owner>) -> Result<(), EncodeError> {
    let Some(owner) = owner else {
        w.u8(0);
        return Ok(());
    };
    w.u8(1);
    w.str(&owner.desk)?;
    match &owner.strategy {
        Some(strategy) => {
            w.u8(1);
            w.str(strategy)?;
        }
        None => {
            w.u8(0);
        }
    }
    Ok(())
}

fn read_owner(r: &mut Reader<'_>) -> Result<Option<Owner>, DecodeError> {
    let flag = |r: &mut Reader<'_>, field| match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(DecodeError::InvalidValue { field, found: u64::from(other) }),
    };
    if !flag(r, "owner")? {
        return Ok(None);
    }
    let desk = r.str()?.to_string();
    let strategy = if flag(r, "owner.strategy")? { Some(r.str()?.to_string()) } else { None };
    Ok(Some(Owner { desk, strategy }))
}

/// Appended after the time, so a record of any other action ends where version 2 ended.
fn write_adjustment(w: &mut Writer, a: &PositionAdjustment) -> Result<(), EncodeError> {
    w.str(&a.instrument)?;
    w.u8(match a.side {
        Side::Buy => SIDE_BUY,
        Side::Sell => SIDE_SELL,
    });
    w.i128(a.quantity_scaled).i128(a.price_scaled);
    w.str(&a.reason)?;
    Ok(())
}

fn read_adjustment(r: &mut Reader<'_>) -> Result<PositionAdjustment, DecodeError> {
    let instrument = r.str()?.to_string();
    let side = match r.u8()? {
        SIDE_BUY => Side::Buy,
        SIDE_SELL => Side::Sell,
        other => return Err(DecodeError::InvalidValue { field: "side", found: u64::from(other) }),
    };
    let quantity_scaled = r.i128()?;
    let price_scaled = r.i128()?;
    let reason = r.str()?.to_string();
    Ok(PositionAdjustment { instrument, side, quantity_scaled, price_scaled, reason, owner: None })
}

fn read_signature(r: &mut Reader<'_>) -> Result<Option<[u8; SIGNATURE_LEN]>, DecodeError> {
    match r.u8()? {
        0 => Ok(None),
        1 => {
            let bytes = r.bytes()?;
            let signature: [u8; SIGNATURE_LEN] = bytes.try_into().map_err(|_| {
                DecodeError::InvalidValue { field: "signature", found: bytes.len() as u64 }
            })?;
            Ok(Some(signature))
        }
        other => Err(DecodeError::InvalidValue { field: "signature", found: u64::from(other) }),
    }
}

pub fn decode(payload: &[u8]) -> Result<Command, DecodeError> {
    decode_signed(payload).map(|(command, _)| command)
}

/// The command and the signature it carried, if any. A record older than version 4 carries none.
pub fn decode_signed(
    payload: &[u8],
) -> Result<(Command, Option<[u8; SIGNATURE_LEN]>), DecodeError> {
    let (envelope, mut r) = Reader::open(payload)?;
    if envelope.version > VERSION {
        return Err(DecodeError::UnsupportedVersion {
            class: envelope.class,
            found: envelope.version,
            known: VERSION,
        });
    }
    if envelope.class != ClassId::OperatorCommand {
        return Err(DecodeError::UnknownClass { found: envelope.class.as_u16() });
    }

    let id = r.str()?.to_string();
    let actor = r.str()?.to_string();
    let role_text = r.str()?;
    // An unknown role is refused: reading it as any role would downgrade or grant a command wrongly.
    let role = Role::named(role_text)
        .ok_or(DecodeError::InvalidValue { field: "role", found: role_text.len() as u64 })?;
    let action = action_from(r.u8()?)?;
    let venue = r.str()?.to_string();
    let at_ns = r.i128()?;
    let instruction = match action {
        Action::Halt => Instruction::Halt,
        Action::Reduce => Instruction::Reduce,
        Action::Resume => Instruction::Resume,
        Action::ChangeLimit => Instruction::ChangeLimit,
        Action::AcknowledgeAlarm => Instruction::AcknowledgeAlarm,
        Action::ApproveRecommendation => Instruction::ApproveRecommendation,
        Action::CancelOrders => Instruction::CancelOrders,
        Action::Flatten => Instruction::Flatten,
        // A venue instruction with no venue names nothing, and the engine must not have to guess.
        Action::WithdrawVenue if !venue.is_empty() => Instruction::WithdrawVenue { venue },
        Action::AdmitVenue if !venue.is_empty() => Instruction::AdmitVenue { venue },
        Action::AdjustPosition if !venue.is_empty() => {
            Instruction::AdjustPosition { venue, adjustment: read_adjustment(&mut r)? }
        }
        _ => {
            return Err(DecodeError::InvalidValue { field: "venue", found: 0 });
        }
    };
    let signature = if envelope.version >= 4 { read_signature(&mut r)? } else { None };
    let mut instruction = instruction;
    if let Instruction::AdjustPosition { adjustment, .. } = &mut instruction
        && envelope.version >= 5
    {
        adjustment.owner = read_owner(&mut r)?;
    }
    r.finish()?;

    Ok((Command { id, actor, role, instruction, at_ns }, signature))
}

#[cfg(test)]
#[path = "../../tests/unit/signal/command_tests.rs"]
mod tests;
