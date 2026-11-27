// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Which venue base names denote one platform asset (`docs/decisions/0006-asset-names.md`).

use std::collections::BTreeMap;

use bijective_common::configuration::{Invalid, Reason};
use bijective_venue::profile::{InstrumentCatalogue, VenueProfile};

/// Each venue base name, in lower case, and the asset it denotes. Only `assets_of` produces one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Assets {
    asset_of: BTreeMap<String, String>,
}

impl Assets {
    /// The asset `base` denotes: its declared asset, or the name itself in lower case.
    #[must_use]
    pub fn canonical(&self, base: &str) -> String {
        let name = base.to_ascii_lowercase();
        self.asset_of.get(&name).cloned().unwrap_or(name)
    }

    /// The profile with each instrument's base rewritten to its asset. Currencies untouched.
    #[must_use]
    pub fn applied(&self, mut profile: VenueProfile) -> VenueProfile {
        let rewritten = profile.instruments.iter().cloned().map(|mut instrument| {
            instrument.base_currency = instrument.base_currency.map(|b| self.canonical(&b));
            instrument
        });
        profile.instruments = InstrumentCatalogue::from_instruments(rewritten);
        profile
    }
}

/// Every name non-empty and denoting one asset, and no asset listed as another's name.
pub(super) fn assets_of(declared: &BTreeMap<String, Vec<String>>) -> Result<Assets, Invalid> {
    let malformed = |detail: String| Invalid::new("assets", Reason::Malformed { detail });
    let mut asset_of = BTreeMap::new();
    for (asset, names) in declared {
        let asset = asset.to_ascii_lowercase();
        if asset.trim().is_empty() || names.iter().any(|n| n.trim().is_empty()) {
            return Err(Invalid::new("assets", Reason::Empty));
        }
        for name in names {
            let name = name.to_ascii_lowercase();
            if let Some(other) = asset_of.insert(name.clone(), asset.clone()) {
                return Err(malformed(format!("{name} is listed under {other} and {asset}")));
            }
        }
    }
    for asset in declared.keys() {
        let asset = asset.to_ascii_lowercase();
        if let Some(other) = asset_of.get(&asset).filter(|other| **other != asset) {
            return Err(malformed(format!("{asset} is declared, and listed as a name of {other}")));
        }
    }
    Ok(Assets { asset_of })
}
