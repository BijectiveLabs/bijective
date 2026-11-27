// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! `bijective`: the operator's command line.

use std::path::PathBuf;
use std::process::ExitCode;

use bijective_authority::Role;
use bijective_cli::{issue, keygen, parse_instruction};
use bijective_serialization::signal::node::{LIVE, PAPER};

const USAGE: &str = "usage:
  bijective keygen <name> <observer|operator|risk-officer> <directory>
  bijective issue [--paper] <store-root> <actor> <role> <key-file> <action> [venue] [words...]
    --paper sends the command to the paper node, never to the live one
    actions: halt | reduce | resume | cancel-orders | flatten | withdraw-venue <venue> | admit-venue <venue>
             adjust-position <venue> <instrument> <buy|sell> <quantity> <price>
                 [--desk <desk> [--strategy <name>]] <reason>";

fn role_of(text: &str) -> Result<Role, String> {
    Role::named(text).ok_or_else(|| format!("{text} is not a role this build knows"))
}

fn run(arguments: &[String]) -> Result<String, String> {
    match arguments {
        [command, name, role, directory] if command == "keygen" => {
            keygen(name, role_of(role)?, &PathBuf::from(directory))
        }
        [command, rest @ ..] if command == "issue" => {
            let (node, rest) = match rest {
                [flag, others @ ..] if flag == "--paper" => (PAPER, others),
                _ => (LIVE, rest),
            };
            let [store, actor, role, key_file, action, rest @ ..] = rest else {
                return Err(USAGE.to_string());
            };
            let (subject, words) = match rest {
                [first, others @ ..] => (Some(first.clone()), others),
                [] => (None, rest),
            };
            let instruction = parse_instruction(action, subject, words)?;
            let signer = (actor.as_str(), role_of(role)?, PathBuf::from(key_file));
            let position = issue(
                &PathBuf::from(store),
                (signer.0, signer.1, signer.2.as_path()),
                instruction,
                node,
            )?;
            Ok(format!("issued at position {position} on {}", node.commands))
        }
        _ => Err(USAGE.to_string()),
    }
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match run(&arguments) {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(detail) => {
            eprintln!("bijective: {detail}");
            ExitCode::from(2)
        }
    }
}
