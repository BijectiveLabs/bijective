// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A static web application served beside the console's pages, under `/app/`, from a directory the
//! operator names. Any application can be served this way: it reads the same `/api` tables and
//! sends commands through the same `/control` form, so it gains no access a page does not have.
//!
//! Only plain names are served: no hidden file, no parent step, no link, no extension outside the
//! list below, and nothing past a size bound. Its policy lets it run its own scripts and
//! WebAssembly and reach this console only.

use std::path::{Path, PathBuf};

/// The address the application is served under.
pub const PREFIX: &str = "/app/";

/// The largest file served. A bundled table engine's WebAssembly is a few megabytes.
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// Path segments below the application directory.
const MAX_DEPTH: usize = 8;

/// The policy of every application answer: its own scripts, workers and WebAssembly, its own
/// styles, and connections to this console only. Nothing from a network, no inline script. A
/// `blob:` script is one the application's own code made in the page, as a table engine does when
/// it unpacks its WebAssembly. It opens no way in that `'self'` does not.
pub const POLICY: &str = "Content-Security-Policy: default-src 'none'; script-src 'self' blob: \
                          'wasm-unsafe-eval'; worker-src 'self' blob:; style-src 'self' \
                          'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src \
                          'self'; form-action 'self'; frame-ancestors 'none'; base-uri \
                          'none'\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Served {
    File {
        content_type: &'static str,
        body: Vec<u8>,
    },
    /// Not a name this console serves, or no such file. The two are not told apart.
    NotFound,
}

/// The media type of a served extension. `None` refuses the file.
#[must_use]
pub fn content_type(path: &Path) -> Option<&'static str> {
    let types = [
        ("html", "text/html; charset=utf-8"),
        ("js", "text/javascript; charset=utf-8"),
        ("mjs", "text/javascript; charset=utf-8"),
        ("css", "text/css; charset=utf-8"),
        ("json", "application/json"),
        ("wasm", "application/wasm"),
        ("svg", "image/svg+xml"),
        ("png", "image/png"),
        ("woff2", "font/woff2"),
        ("txt", "text/plain; charset=utf-8"),
    ];
    let extension = path.extension()?.to_str()?;
    types.iter().find(|(e, _)| *e == extension).map(|(_, t)| *t)
}

/// The file `route` names under `root`, or `None` when the route is not a plain name. A plain
/// segment is letters, digits, `.`, `-`, `_` and `@` (a package scope, as bundled libraries lay
/// their files out), and does not begin with a dot.
#[must_use]
pub fn resolve(root: &Path, route: &str) -> Option<PathBuf> {
    let route = route.split(['?', '#']).next()?;
    let relative = route.strip_prefix(PREFIX)?;
    let relative = if relative.is_empty() { "index.html" } else { relative };
    let segments: Vec<&str> = relative.split('/').collect();
    let plain = |s: &&str| {
        !s.is_empty()
            && !s.starts_with('.')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@'))
    };
    if segments.len() > MAX_DEPTH || !segments.iter().all(plain) {
        return None;
    }
    Some(segments.iter().fold(root.to_path_buf(), |path, s| path.join(s)))
}

/// What `route` answers. A link is not followed and a file past the bound is not read.
#[must_use]
pub fn serve(root: &Path, route: &str) -> Served {
    let Some(path) = resolve(root, route) else { return Served::NotFound };
    let Some(content_type) = content_type(&path) else { return Served::NotFound };
    let Ok(metadata) = std::fs::symlink_metadata(&path) else { return Served::NotFound };
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Served::NotFound;
    }
    // A linked directory on the way could lead outside the application.
    let inside = match (path.canonicalize(), root.canonicalize()) {
        (Ok(file), Ok(root)) => file.starts_with(root),
        _ => false,
    };
    if !inside {
        return Served::NotFound;
    }
    match std::fs::read(&path) {
        Ok(body) => Served::File { content_type, body },
        Err(_) => Served::NotFound,
    }
}

#[cfg(test)]
#[path = "../tests/unit/app_tests.rs"]
mod tests;
