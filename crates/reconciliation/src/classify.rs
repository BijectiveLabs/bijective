// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Matching, then classification. Every divergence lands in exactly one named class.

use crate::record::{FallbackKey, Fidelity, PlatformPosition, VenuePosition};
use bijective_model::VenueSymbol;

/// How a pair was established. Carried, never inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    Exact,
    /// Paired by the profile's declared fallback. Recorded so a weaker match is never read as exact.
    Fallback(FallbackKey),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DivergenceClass {
    /// In flight inside the profile's timing window. Recomputed every run, not assigned once.
    InFlight,
    /// We hold a record the venue does not. Ours is never deleted.
    Missing,
    /// Venue-only record. Most severe: capital moved and the platform did not see it.
    Extra,
    /// Both hold it and a field differs. Both values are shown. Neither is adopted.
    Value { field: &'static str, ours: i128, theirs: i128 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub class: DivergenceClass,
    pub instrument: VenueSymbol,
    /// Sequence position in our stream, never wall-clock time.
    pub as_of: u64,
    pub raised_at_ns: i128,
    pub match_kind: Option<MatchKind>,
}

/// Venue record that cannot be paired at its declared fidelity. Kept apart from divergences: the fault is in the profile, not the book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileDefect {
    pub instrument: VenueSymbol,
    pub detail: String,
    pub as_of: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub as_of: u64,
    pub matched: usize,
    pub divergences: Vec<Divergence>,
    /// Routed to venue assurance. Kept separate from `divergences` by construction.
    pub profile_defects: Vec<ProfileDefect>,
}

impl Report {
    pub fn is_clear(&self) -> bool {
        self.divergences.is_empty() && self.profile_defects.is_empty()
    }
}

/// Reconcile both sides as of a sequence position. Borrows both, so there is no mutable path to either.
pub fn reconcile(
    ours: &[PlatformPosition],
    theirs: &[VenuePosition],
    fidelity: Fidelity,
    in_flight_window_ns: i128,
    as_of: u64,
    now_ns: i128,
) -> Report {
    let mut divergences = Vec::new();
    let mut profile_defects = Vec::new();
    let mut matched = 0;
    let mut paired_theirs = vec![false; theirs.len()];

    for mine in ours {
        match find_pair(mine, theirs, fidelity, &paired_theirs) {
            Some((index, kind)) => {
                paired_theirs[index] = true;
                matched += 1;
                divergences.extend(compare(mine, &theirs[index], kind, as_of, now_ns));
            }
            None => {
                let class = if now_ns - mine.opened_at_ns < in_flight_window_ns {
                    DivergenceClass::InFlight
                } else {
                    DivergenceClass::Missing
                };
                divergences.push(Divergence {
                    class,
                    instrument: mine.instrument.clone(),
                    as_of,
                    raised_at_ns: now_ns,
                    match_kind: None,
                });
            }
        }
    }

    for (index, theirs_row) in theirs.iter().enumerate() {
        if paired_theirs[index] {
            continue;
        }

        if !pairable(theirs_row, fidelity) {
            profile_defects.push(ProfileDefect {
                instrument: theirs_row.instrument.clone(),
                detail: describe_unpairable(theirs_row, fidelity),
                as_of,
            });
            continue;
        }

        // In-flight is judged on the venue's clock: the only timestamp for a record we never saw.
        let venue_ns = theirs_row.updated_at_ms as i128 * 1_000_000;
        let class = if now_ns - venue_ns < in_flight_window_ns {
            DivergenceClass::InFlight
        } else {
            DivergenceClass::Extra
        };
        divergences.push(Divergence {
            class,
            instrument: theirs_row.instrument.clone(),
            as_of,
            raised_at_ns: now_ns,
            match_kind: None,
        });
    }

    Report { as_of, matched, divergences, profile_defects }
}

fn compare(
    mine: &PlatformPosition,
    theirs: &VenuePosition,
    kind: MatchKind,
    as_of: u64,
    now_ns: i128,
) -> Vec<Divergence> {
    let mut out = Vec::new();
    let mut differ = |field: &'static str, ours: i128, theirs_value: i128| {
        if ours != theirs_value {
            out.push(Divergence {
                class: DivergenceClass::Value { field, ours, theirs: theirs_value },
                instrument: mine.instrument.clone(),
                as_of,
                raised_at_ns: now_ns,
                match_kind: Some(kind),
            });
        }
    };

    differ("quantity", mine.quantity_scaled, theirs.quantity_scaled);
    differ("average_price", mine.average_price_scaled, theirs.average_price_scaled);
    out
}

fn find_pair(
    mine: &PlatformPosition,
    theirs: &[VenuePosition],
    fidelity: Fidelity,
    taken: &[bool],
) -> Option<(usize, MatchKind)> {
    theirs.iter().enumerate().find_map(|(index, row)| {
        if taken[index] {
            return None;
        }
        match fidelity {
            Fidelity::VenueIdentifier => {
                let ours = mine.venue_order_id.as_deref()?;
                (row.venue_order_id.as_deref() == Some(ours)).then_some((index, MatchKind::Exact))
            }
            Fidelity::Fallback(FallbackKey::ClientOrderId) => (row.client_order_id.as_deref()
                == Some(mine.client_order_id.as_str()))
            .then_some((index, MatchKind::Fallback(FallbackKey::ClientOrderId))),
            Fidelity::Fallback(FallbackKey::Instrument) => (row.instrument == mine.instrument)
                .then_some((index, MatchKind::Fallback(FallbackKey::Instrument))),
            Fidelity::Undeclared => None,
        }
    })
}

fn pairable(row: &VenuePosition, fidelity: Fidelity) -> bool {
    match fidelity {
        Fidelity::VenueIdentifier => row.venue_order_id.is_some(),
        Fidelity::Fallback(FallbackKey::ClientOrderId) => row.client_order_id.is_some(),
        Fidelity::Fallback(FallbackKey::Instrument) => !row.instrument.is_empty(),
        Fidelity::Undeclared => false,
    }
}

fn describe_unpairable(row: &VenuePosition, fidelity: Fidelity) -> String {
    match fidelity {
        Fidelity::VenueIdentifier => {
            format!("the profile declares venue identifiers, and {} carries none", row.instrument)
        }
        Fidelity::Fallback(FallbackKey::ClientOrderId) => format!(
            "the profile declares a client-identifier fallback, and {} carries none",
            row.instrument
        ),
        Fidelity::Fallback(FallbackKey::Instrument) => {
            "the profile declares an instrument fallback, and the record names no instrument"
                .to_string()
        }
        Fidelity::Undeclared => {
            "the profile declares no way to pair this venue's records".to_string()
        }
    }
}
