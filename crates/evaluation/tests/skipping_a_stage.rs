// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A promotion that skips a stage does not compile. The expression is handed to
//! the compiler as text. The positive case runs too, so "does not compile" is not true of everything.

use std::process::Command;

fn compiles(body: &str) -> (bool, String) {
    let dir = std::env::temp_dir().join(format!(
        "bijective-promotion-skip-{}-{}",
        std::process::id(),
        body.len()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("a place to write");

    let here = env!("CARGO_MANIFEST_DIR");
    let store = format!("{here}/../event_store");
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"skipcheck\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             [dependencies]\n\
             bijective-evaluation = {{ path = \"{here}\" }}\n\
             bijective-event-store = {{ path = \"{store}\" }}\n",
        ),
    )
    .expect("manifest");
    std::fs::write(
        dir.join("src/main.rs"),
        format!(
            "use bijective_event_store::digest::Digest;\n\
             use bijective_evaluation::promotion::*;\n\
             fn main() {{\n\
             let d = Promoted {{ run: Digest([0u8; 32]), strategy: Some(Digest([1u8; 32])), content: Some(Digest([2u8; 32])), held_out_positions: 1 }};\n\
             {body}\n}}\n"
        ),
    )
    .expect("source");

    let out = Command::new(env!("CARGO"))
        .args(["build", "--offline", "--quiet"])
        .current_dir(&dir)
        .output()
        .expect("cargo runs");
    let said = String::from_utf8_lossy(&out.stderr).to_string();
    let _ = std::fs::remove_dir_all(&dir);
    (out.status.success(), said)
}

#[test]
fn the_required_sequence_compiles_and_skipping_a_stage_does_not() {
    let (built, said) = compiles(
        "let back = Cleared::<Backtest>::first(d).expect(\"a strategy\");\n\
         let paper = Cleared::<Paper>::after(back, d).expect(\"the same strategy\");\n\
         let _ = Cleared::<Live>::after(paper, d);",
    );
    assert!(built, "the sequence the domain requires does not compile: {said}");

    let (built, said) = compiles(
        "let back = Cleared::<Backtest>::first(d).expect(\"a strategy\");\n\
         let _ = Cleared::<Live>::after(back, d);",
    );
    assert!(!built, "a live promotion straight off a backtest compiled");
    assert!(
        said.contains("Cleared<Paper>"),
        "it failed for some reason other than the missing stage: {said}"
    );
}
