// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What the record says about the order things happened in, for an operator reading a node.
//! Usage: bijective-check-ordering <store> [control-stream] [proposal-stream]

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (root, control, proposals) = match a.as_slice() {
        [root] => (root.clone(), "control".to_string(), "proposals".to_string()),
        [root, control, proposals] => (root.clone(), control.clone(), proposals.clone()),
        _ => {
            eprintln!("usage: bijective-check-ordering <store> [control-stream] [proposal-stream]");
            std::process::exit(2);
        }
    };

    match bijective_ordering::audit(std::path::Path::new(&root), &control, &proposals) {
        Ok((read, findings)) if findings.is_empty() => {
            println!("{read} control record(s) read, nothing out of order");
        }
        Ok((read, findings)) => {
            println!("{read} control record(s) read, {} finding(s):", findings.len());
            for finding in &findings {
                println!("  {finding}");
            }
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
