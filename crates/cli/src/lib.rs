// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The operator's command line: key generation and signed commands.
//!
//! `issue` appends one signed command to the `commands` stream. The platform believes it only if
//! the name it carries is in the operator registry and the signature verifies under that
//! operator's public key. The private key never leaves the file it is generated into.

use std::io::Write;
use std::path::Path;

use bijective_authority::Role;
use bijective_event_store::store::StreamWriter;
use bijective_identity::{SEED_LEN, SigningKey, from_hex_32, to_hex};
use bijective_model::{Owner, Side};
use bijective_serialization::signal::command::{
    Command, Instruction, PositionAdjustment, encode, encode_signed,
};
use bijective_serialization::signal::node::NodeStreams;

/// The action named on the command line, with the venue it is about and any further words.
pub fn parse_instruction(
    action: &str,
    subject: Option<String>,
    rest: &[String],
) -> Result<Instruction, String> {
    match (action, subject) {
        ("halt", _) => Ok(Instruction::Halt),
        ("reduce", _) => Ok(Instruction::Reduce),
        ("resume", _) => Ok(Instruction::Resume),
        ("cancel-orders", _) => Ok(Instruction::CancelOrders),
        ("flatten", _) => Ok(Instruction::Flatten),
        ("withdraw-venue", Some(venue)) => Ok(Instruction::WithdrawVenue { venue }),
        ("admit-venue", Some(venue)) => Ok(Instruction::AdmitVenue { venue }),
        ("adjust-position", Some(venue)) => {
            Ok(Instruction::AdjustPosition { venue, adjustment: adjustment(rest)? })
        }
        // Refused rather than defaulted to every venue: the engine would have to guess the subject.
        ("withdraw-venue" | "admit-venue" | "adjust-position", None) => {
            Err(format!("{action} needs the venue it is about"))
        }
        (other, _) => Err(format!("{other} is not an action this tool issues")),
    }
}

/// Scaled integers as given: nothing is converted, so what is recorded is what was typed.
fn adjustment(rest: &[String]) -> Result<PositionAdjustment, String> {
    let [instrument, side, quantity, price, reason @ ..] = rest else {
        return Err("needs <instrument> <buy|sell> <quantity> <price> <reason>".into());
    };
    let side = match side.as_str() {
        "buy" => Side::Buy,
        "sell" => Side::Sell,
        other => return Err(format!("{other} is not buy or sell")),
    };
    let whole = |v: &str| v.parse::<i128>().map_err(|_| format!("{v} is not a scaled integer"));
    let (owner, reason) = owner_of(reason)?;
    Ok(PositionAdjustment {
        instrument: instrument.clone(),
        side,
        quantity_scaled: whole(quantity)?,
        price_scaled: whole(price)?,
        reason: reason.join(" "),
        owner,
    })
}

/// `--desk <desk>` and then `--strategy <name>`, where given, ahead of the reason. A strategy
/// without a desk is refused: an owner is a desk first.
fn owner_of(words: &[String]) -> Result<(Option<Owner>, &[String]), String> {
    let [flag, desk, rest @ ..] = words else { return Ok((None, words)) };
    if flag == "--strategy" {
        return Err("--strategy needs --desk before it".into());
    }
    if flag != "--desk" {
        return Ok((None, words));
    }
    match rest {
        [flag, strategy, reason @ ..] if flag == "--strategy" => {
            Ok((Some(Owner { desk: desk.clone(), strategy: Some(strategy.clone()) }), reason))
        }
        _ => Ok((Some(Owner { desk: desk.clone(), strategy: None }), rest)),
    }
}

/// Writes `<name>.key` (the seed, hex, owner-only) and returns the registry entry for the public
/// half. Refuses to replace a key that exists: a lost key cannot be told from a replaced one.
pub fn keygen(name: &str, role: Role, directory: &Path) -> Result<String, String> {
    let (seed, key) = SigningKey::generate().map_err(|e| e.to_string())?;
    let path = directory.join(format!("{name}.key"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    writeln!(file, "{}", to_hex(&seed)).map_err(|e| e.to_string())?;
    Ok(format!(
        r#"{{ "name": "{name}", "role": "{}", "public_key": "{}" }}"#,
        role.as_str(),
        to_hex(&key.public_key())
    ))
}

/// The signing key in a file `keygen` wrote. Read when it is needed, not held, so a long-running
/// caller keeps the seed in memory only for the moment it signs.
pub fn load_key(key_file: &Path) -> Result<SigningKey, String> {
    let text =
        std::fs::read_to_string(key_file).map_err(|e| format!("{}: {e}", key_file.display()))?;
    let seed: [u8; SEED_LEN] = from_hex_32(&text).map_err(|e| e.to_string())?;
    SigningKey::from_seed(&seed).map_err(|e| e.to_string())
}

/// Signs and appends one command. Returns the position it landed at.
pub fn issue(
    store_root: &Path,
    signer: (&str, Role, &Path),
    instruction: Instruction,
    node: NodeStreams,
) -> Result<u64, String> {
    let (actor, role, key_file) = signer;
    let key = load_key(key_file)?;

    let mut writer = StreamWriter::open(store_root, node.commands)
        .map_err(|e| format!("the command stream could not be opened: {e}"))?;
    // The identifier is the position it will land at: unique without a clock or random source.
    let id = format!("cmd-{}", writer.next_sequence());
    let command = Command { id, actor: actor.to_string(), role, instruction, at_ns: 0 };

    let message = encode(&command).map_err(|e| format!("{e:?}"))?;
    let bytes = encode_signed(&command, &key.sign(&message)).map_err(|e| format!("{e:?}"))?;
    writer.append(&bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
