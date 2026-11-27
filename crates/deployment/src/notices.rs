// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Third-party notices, generated from the attestation (`Release_Gate.md` L1).
//! `NOTICES.md` explains why election is a decision.

use std::collections::BTreeMap;
use std::path::Path;

/// One attested package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Attested {
    pub name: String,
    pub version: String,
    pub licence: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticesError {
    AttestationUnreadable,
    Malformed {
        line: usize,
    },
    Empty,
    /// Every offered option is one we do not elect. Refused rather than skipped.
    NoElection {
        package: String,
        offered: String,
    },
    /// A licence is named and no text was found. Refused: omitting it would look like discharge.
    TextMissing {
        licence: String,
    },
}

impl core::fmt::Display for NoticesError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AttestationUnreadable => write!(f, "the attestation could not be read"),
            Self::Malformed { line } => write!(f, "attestation line {line} is not three fields"),
            Self::Empty => write!(f, "the attestation lists no packages"),
            Self::NoElection { package, offered } => {
                write!(f, "{package} offers only {offered}, none of which we elect")
            }
            Self::TextMissing { licence } => {
                write!(f, "no text was found for {licence}, so its notice cannot be discharged")
            }
        }
    }
}

impl std::error::Error for NoticesError {}

pub fn parse_attestation(raw: &str) -> Result<Vec<Attested>, NoticesError> {
    let mut out = Vec::new();
    for (n, line) in raw.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(name), Some(version), Some(licence), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(NoticesError::Malformed { line: n + 1 });
        };
        out.push(Attested {
            name: name.to_string(),
            version: version.to_string(),
            licence: licence.to_string(),
        });
    }
    if out.is_empty() {
        return Err(NoticesError::Empty);
    }
    out.sort();
    Ok(out)
}

/// The licence we take when a package offers a choice, in order of preference.
/// Not the allow-list: a licence allow-listed but missing here fails generation by name.
const ELECTION_ORDER: [&str; 11] = [
    "MIT",
    "Apache-2.0",
    "ISC",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "0BSD",
    "Zlib",
    "Unlicense",
    "CC0-1.0",
    "CDLA-Permissive-2.0",
    "Unicode-3.0",
];

/// The licences an SPDX expression obliges us to notice: one elected option per `AND` conjunct.
pub fn elected(expression: &str) -> Vec<String> {
    let mut out = Vec::new();

    for conjunct in expression.split(" AND ") {
        let cleaned = conjunct.replace(['(', ')'], "");
        let tokens: Vec<&str> = cleaned
            .split(&[' ', '/'][..])
            .map(str::trim)
            .filter(|t| !t.is_empty() && *t != "OR" && *t != "WITH")
            .collect();

        if tokens.is_empty() {
            continue;
        }

        // Required even without a choice, so a copyleft-only package fails here and not later as a missing text.
        match ELECTION_ORDER.iter().find(|p| tokens.contains(p)) {
            Some(elected) => out.push((*elected).to_string()),
            // Nothing elected makes the whole expression unusable. `render` refuses an empty result.
            None => return Vec::new(),
        }
    }

    out.sort();
    out.dedup();
    out
}

pub fn distinct_licences(packages: &[Attested]) -> Vec<String> {
    let mut seen: Vec<String> = packages.iter().flat_map(|p| elected(&p.licence)).collect();
    seen.sort();
    seen.dedup();
    seen
}

/// Deterministic: sorted, no timestamp, no host name, so the build check is a diff.
/// One copy of each distinct licence text, not one per package.
pub fn render(
    packages: &[Attested],
    texts: &BTreeMap<String, String>,
) -> Result<String, NoticesError> {
    let mut out = String::new();
    out.push_str("# Third-Party Licences\n\n");
    out.push_str("Generated from the dependency attestation. Do not edit by hand - the build\n");
    out.push_str("check regenerates this file and refuses a difference.\n\n");
    out.push_str(&format!("{} package(s).\n\n", packages.len()));

    out.push_str("## Packages\n\n");
    out.push_str("| Package | Version | Offered | Elected |\n|---|---|---|---|\n");
    for p in packages {
        let elected = elected(&p.licence);
        if elected.is_empty() {
            return Err(NoticesError::NoElection {
                package: format!("{} {}", p.name, p.version),
                offered: p.licence.clone(),
            });
        }
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            p.name,
            p.version,
            p.licence,
            elected.join(", ")
        ));
    }

    out.push_str("\n## Licence texts\n\n");
    for licence in distinct_licences(packages) {
        let text =
            texts.get(&licence).ok_or(NoticesError::TextMissing { licence: licence.clone() })?;
        out.push_str(&format!("### {licence}\n\n```\n{}\n```\n\n", text.trim_end()));
    }
    Ok(out)
}

/// One text per licence, from the fallback directory or a carrying crate's own LICENSE file.
pub fn collect_texts(
    packages: &[Attested],
    registry: &Path,
    fallback: &Path,
) -> BTreeMap<String, String> {
    let mut texts = BTreeMap::new();

    for licence in distinct_licences(packages) {
        if let Some(text) = read_named(fallback, &licence) {
            texts.insert(licence, text);
            continue;
        }
        for p in packages.iter().filter(|p| elected(&p.licence).contains(&licence)) {
            if let Some(text) = read_from_crate(registry, &p.name, &p.version) {
                texts.insert(licence.clone(), text);
                break;
            }
        }
    }
    texts
}

fn read_named(dir: &Path, licence: &str) -> Option<String> {
    let path = dir.join(format!("{licence}.txt"));
    std::fs::read_to_string(path).ok()
}

fn read_from_crate(registry: &Path, name: &str, version: &str) -> Option<String> {
    let entries = std::fs::read_dir(registry).ok()?;
    for index in entries.flatten() {
        let dir = index.path().join(format!("{name}-{version}"));
        for candidate in ["LICENSE", "LICENSE-MIT", "LICENSE-APACHE", "LICENSE.md", "COPYING"] {
            if let Ok(text) = std::fs::read_to_string(dir.join(candidate))
                && !text.trim().is_empty()
            {
                return Some(text);
            }
        }
    }
    None
}
