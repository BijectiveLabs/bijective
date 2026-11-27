// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Only plain names inside the application directory are served.

use super::*;

fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-console-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("assets")).expect("scratch");
    std::fs::write(dir.join("index.html"), "<!doctype html>").expect("index");
    std::fs::write(dir.join("assets").join("app.js"), "export {}").expect("script");
    std::fs::write(dir.join("assets").join("notes.exe"), "x").expect("other");
    std::fs::write(dir.join(".env"), "SECRET=1").expect("hidden");
    std::fs::create_dir_all(dir.join("vendor").join("@scope")).expect("scope");
    std::fs::write(dir.join("vendor").join("@scope").join("engine.wasm"), [0u8; 4]).expect("wasm");
    dir
}

#[test]
fn the_index_and_plain_names_are_served_with_their_type() {
    let dir = root("plain");
    for (route, expected) in [
        ("/app/", "text/html; charset=utf-8"),
        ("/app/index.html?v=1", "text/html; charset=utf-8"),
        ("/app/assets/app.js", "text/javascript; charset=utf-8"),
        ("/app/vendor/@scope/engine.wasm", "application/wasm"),
    ] {
        match serve(&dir, route) {
            Served::File { content_type, .. } => assert_eq!(content_type, expected, "{route}"),
            Served::NotFound => panic!("{route} was not served"),
        }
    }
}

#[test]
fn nothing_outside_plain_names_is_served() {
    let dir = root("refused");
    for route in [
        "/app/../Cargo.toml",
        "/app/assets/../../x.js",
        "/app/.env",
        "/app/assets/notes.exe",
        "/app//index.html",
        "/app/%2e%2e/x.js",
        "/app/missing.js",
        "/apps/index.html",
        "/app/assets",
    ] {
        assert_eq!(serve(&dir, route), Served::NotFound, "{route}");
    }
}

#[cfg(unix)]
#[test]
fn a_link_out_of_the_application_is_not_followed() {
    let dir = root("link");
    let outside = root("link-outside");
    std::os::unix::fs::symlink(outside.join("assets"), dir.join("linked")).expect("link");
    std::os::unix::fs::symlink(outside.join("index.html"), dir.join("file.html")).expect("link");
    assert_eq!(serve(&dir, "/app/linked/app.js"), Served::NotFound);
    assert_eq!(serve(&dir, "/app/file.html"), Served::NotFound);
}

#[test]
fn the_policy_lets_the_application_run_only_its_own_code() {
    assert!(POLICY.contains("script-src 'self' blob: 'wasm-unsafe-eval';"));
    let scripts = POLICY.split(';').find(|d| d.trim().starts_with("script-src")).expect("scripts");
    assert!(!scripts.contains("unsafe-inline"), "no inline script: {scripts}");
    assert!(POLICY.contains("connect-src 'self';"));
    assert!(!POLICY.contains(" 'unsafe-eval'"), "no evaluation of text as code");
    assert!(!POLICY.contains("http"), "no network source");
}
