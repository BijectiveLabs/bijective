// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

use super::*;

fn command() -> Command {
    Command {
        id: "cmd-1".into(),
        actor: "duty-operator".into(),
        role: Role::Operator,
        instruction: Instruction::Resume,
        at_ns: 42,
    }
}

/// Every action and role round-trips, and encoding is stable.
#[test]
fn a_command_round_trips_and_encodes_the_same_way_twice() {
    let instructions = [
        Instruction::Halt,
        Instruction::Reduce,
        Instruction::Resume,
        Instruction::ChangeLimit,
        Instruction::AcknowledgeAlarm,
        Instruction::ApproveRecommendation,
        Instruction::CancelOrders,
        Instruction::Flatten,
        Instruction::WithdrawVenue { venue: "venue-a".into() },
        Instruction::AdmitVenue { venue: "venue-a".into() },
        Instruction::AdjustPosition {
            venue: "venue-a".into(),
            adjustment: PositionAdjustment {
                instrument: "AAA".into(),
                side: bijective_model::Side::Sell,
                quantity_scaled: 7,
                price_scaled: 1_000,
                reason: "held before the platform".into(),
                owner: None,
            },
        },
    ];
    // Every action this build knows is in the list above, so a new one cannot go untested.
    assert_eq!(instructions.len(), bijective_authority::Action::ALL.len());
    let roles = [Role::Observer, Role::Operator, Role::RiskOfficer];

    for instruction in instructions {
        for role in roles {
            let shape = Command { instruction: instruction.clone(), role, ..command() };
            let first = encode(&shape).expect("encode");
            assert_eq!(first, encode(&shape).expect("encode"));
            assert_eq!(decode(&first).expect("decode"), shape);
        }
    }
}

/// A role this build does not know is refused, never read as the floor.
#[test]
fn an_unknown_role_is_refused() {
    let mut w = Writer::new(ClassId::OperatorCommand, VERSION);
    w.str("cmd-1").expect("write");
    w.str("someone").expect("write");
    w.str("administrator").expect("write");
    w.u8(0);
    w.i128(1);

    assert!(matches!(decode(&w.finish()), Err(DecodeError::InvalidValue { field: "role", .. })));
}

/// The role is written as its name, so a record reads without this build.
#[test]
fn the_role_travels_as_its_name() {
    let bytes = encode(&command()).expect("encode");

    assert!(
        String::from_utf8_lossy(&bytes).contains("operator"),
        "an auditor should not need this build to say what role 2 was"
    );
}

#[test]
fn a_foreign_class_is_refused() {
    let mut w = Writer::new(ClassId::ProposalEmitted, VERSION);
    w.str("x").expect("write");

    assert!(decode(&w.finish()).is_err());
}

/// An instruction about a venue that names none is refused, never read as *every venue*.
/// The variant makes it unconstructible, so it can only be forged on the wire.
#[test]
fn a_venue_instruction_with_no_venue_is_refused_on_the_wire() {
    let good = Command {
        instruction: Instruction::WithdrawVenue { venue: "venue-a".into() },
        ..command()
    };
    let bytes = encode(&good).expect("encode");

    // The venue is the last string before the timestamp. Blanking it is the forgery this refuses.
    let empty =
        Command { instruction: Instruction::WithdrawVenue { venue: String::new() }, ..command() };
    let forged = encode(&empty).expect("encode");

    assert!(decode(&bytes).is_ok(), "a named venue still round-trips");
    assert!(
        decode(&forged).is_err(),
        "a withdrawal naming no venue was accepted, and the engine would have had to guess"
    );
}

/// The signature rides after the fields and is not part of what it signs: the signed message is
/// the unsigned encoding, which the verifier rebuilds from the decoded command.
#[test]
fn a_signature_travels_with_a_command_and_is_not_part_of_what_it_signs() {
    let signature = [7u8; SIGNATURE_LEN];
    let signed = encode_signed(&command(), &signature).expect("encode");

    let (decoded, found) = decode_signed(&signed).expect("decode");
    assert_eq!(decoded, command());
    assert_eq!(found, Some(signature));
    assert_eq!(encode(&decoded).expect("encode"), encode(&command()).expect("encode"));
    assert_eq!(decode(&signed).expect("decode"), command(), "the plain reader ignores it");
}

#[test]
fn an_unsigned_command_decodes_with_no_signature() {
    let (decoded, found) = decode_signed(&encode(&command()).expect("encode")).expect("decode");
    assert_eq!((decoded, found), (command(), None));
}

/// A record written before signatures existed is still readable, and it is unsigned.
#[test]
fn a_version_three_record_decodes_as_unsigned() {
    let mut payload = encode(&command()).expect("encode");
    payload.truncate(payload.len() - 1);
    payload[2..4].copy_from_slice(&3u16.to_be_bytes());
    let (decoded, found) = decode_signed(&payload).expect("a version-3 record");
    assert_eq!((decoded, found), (command(), None));
}

#[test]
fn a_signature_flag_that_is_neither_zero_nor_one_is_refused() {
    let mut payload = encode(&command()).expect("encode");
    let last = payload.len() - 1;
    payload[last] = 2;
    assert!(decode_signed(&payload).is_err());
}

fn adjusting(owner: Option<bijective_model::Owner>) -> Command {
    Command {
        role: Role::RiskOfficer,
        instruction: Instruction::AdjustPosition {
            venue: "venue-a".into(),
            adjustment: PositionAdjustment {
                instrument: "AAA".into(),
                side: bijective_model::Side::Buy,
                quantity_scaled: 7,
                price_scaled: 1_000,
                reason: "moved in by transfer".into(),
                owner,
            },
        },
        ..command()
    }
}

/// The owner of an adjustment round-trips signed, and is part of what the signature covers.
#[test]
fn an_adjustment_carries_its_owner_inside_what_is_signed() {
    let owner = bijective_model::Owner { desk: "alpha".into(), strategy: Some("trend".into()) };
    let owned = adjusting(Some(owner));
    let signature = [7u8; SIGNATURE_LEN];

    let (decoded, found) =
        decode_signed(&encode_signed(&owned, &signature).expect("encode")).expect("decode");
    assert_eq!((decoded, found), (owned.clone(), Some(signature)));
    assert_ne!(
        encode(&owned).expect("encode"),
        encode(&adjusting(None)).expect("encode"),
        "a signature over the unsigned encoding would not cover the owner"
    );
    let deskonly = adjusting(Some(bijective_model::Owner { desk: "alpha".into(), strategy: None }));
    assert_eq!(decode(&encode(&deskonly).expect("encode")).expect("decode"), deskonly);
}

/// An adjustment written before adjustments named an owner reads as nobody's.
#[test]
fn a_version_four_adjustment_reads_with_no_owner() {
    let mut payload = encode(&adjusting(None)).expect("encode");
    payload.truncate(payload.len() - 1);
    payload[2..4].copy_from_slice(&4u16.to_be_bytes());
    assert_eq!(decode(&payload).expect("a version-4 record"), adjusting(None));
}
