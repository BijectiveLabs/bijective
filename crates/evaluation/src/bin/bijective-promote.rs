// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! How far a strategy has come towards capital, and what stops it going further.
//! Usage: bijective-promote <store> <strategy-program> <confidence-in-the-fixed-scale>

use bijective_evaluation::promotion::steps::{Known, resolve, walk};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [root, program, confidence] = a.as_slice() else {
        eprintln!("usage: bijective-promote <store> <strategy-program> <confidence>");
        std::process::exit(2);
    };
    let root = std::path::Path::new(root);
    let confidence: i128 = confidence.parse().expect("a critical value in the fixed scale");

    let (strategy, known) = resolve(root, program);
    println!("{program}: strategy {}", &strategy.to_hex()[..16]);
    match known {
        Known::ByFileName => println!("  (found by the name its file suggests)"),
        Known::ByPath => println!("  (found by its path; nothing has run under that name)"),
        _ => {}
    }
    for step in walk(root, program, confidence) {
        println!("  {:<9} {}", format!("{}:", step.stage), step.said);
        if let Some(next) = step.next {
            println!("  next:     {next}");
        }
    }
}
