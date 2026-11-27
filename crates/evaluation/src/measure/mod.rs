// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a run's proposals cost before they earn anything: spread, fees and gross return at the declared horizon.
//! Prices recorded proposals against the recorded book. `docs/concepts/evaluation.md` lists what it does not measure.

#[cfg(test)]
#[path = "../../tests/unit/measure/tests.rs"]
mod tests;

use std::path::Path;

use crate::evidence::registry::{REGISTRATIONS, standing};
use crate::evidence::{Fold, Horizon, Measured, Positions};
use bijective_event_store::digest::Digest;
use bijective_event_store::projection::Recovered;
use bijective_event_store::store::read_range;
use bijective_model::{Proposal, Side};
use bijective_serialization::signal::fees::{Fees, standing as fees_standing};
use bijective_serialization::signal::market::{BookUpdate, venue_of};
use bijective_strategy_engine::ledger::{Ledger, RUNS};

mod book;
use book::*;
mod arithmetic;
use arithmetic::*;

mod pricing;
use pricing::*;

pub const ONE: i128 = 10_000;

const BPS: i128 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub proposal_position: u64,
    pub market_position: u64,
    pub spread: i128,
    /// `None` where the claim declared no horizon. Not a return of zero.
    pub gross: Option<i128>,
    /// A limit resting behind the touch may never fill, and would size evidence on trades that did not happen.
    pub marketable: bool,
}

/// A proposal that could not be priced, and why. Never dropped, never defaulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unplaced {
    MarketMissing {
        proposal_position: u64,
        market_position: u64,
    },
    MarketUnreadable {
        market_position: u64,
    },
    ProposalUnreadable {
        proposal_position: u64,
    },
    OtherInstrument {
        proposal_position: u64,
        proposed: String,
        book: String,
    },
    NoMid {
        market_position: u64,
    },
    /// The range holds more positions than the admission wrote, so proposals cannot be attributed.
    Interleaved {
        from: u64,
        through: u64,
        count: u64,
    },
    /// The horizon reaches past the market stream. Not a shorter hold: that would answer an undeclared horizon.
    HorizonBeyondTheRecord {
        from: u64,
        wanted: u64,
    },
}

impl core::fmt::Display for Placed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "proposal {} against market {}: spread {} bp/{ONE}{}",
            self.proposal_position,
            self.market_position,
            self.spread,
            if self.marketable { "" } else { ", behind the touch" }
        )
    }
}

impl core::fmt::Display for Unplaced {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Unplaced::MarketMissing { proposal_position, market_position } => write!(
                f,
                "proposal {proposal_position}: market position {market_position} holds nothing"
            ),
            Unplaced::MarketUnreadable { market_position } => {
                write!(f, "the market at {market_position} is not one this build can read")
            }
            Unplaced::ProposalUnreadable { proposal_position } => {
                write!(f, "the proposal at {proposal_position} is not one this build can read")
            }
            Unplaced::OtherInstrument { proposal_position, proposed, book } => {
                write!(f, "proposal {proposal_position} names {proposed} and the book is {book}")
            }
            Unplaced::NoMid { market_position } => {
                write!(f, "the book at {market_position} has no mid to measure against")
            }
            Unplaced::Interleaved { from, through, count } => write!(
                f,
                "positions {from}..={through} hold more than the {count} this run wrote, so none of them is attributed"
            ),
            Unplaced::HorizonBeyondTheRecord { from, wanted } => write!(
                f,
                "the horizon from {from} reaches {wanted}, which the market stream does not hold"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Floor {
    Measured {
        round_trip: i128,
    },
    /// The spread is measured and something else is not. Not a smaller floor: a floor missing its fee
    /// has not been cleared.
    Unavailable {
        spread_round_trip: i128,
        reason: String,
    },
}

impl core::fmt::Display for Floor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Floor::Measured { round_trip } => {
                write!(f, "{round_trip} bp/{ONE} round trip, spread and fee")
            }
            Floor::Unavailable { spread_round_trip, reason } => write!(
                f,
                "unavailable ({reason}); the spread alone is {spread_round_trip} bp/{ONE} round trip"
            ),
        }
    }
}

/// Whether the measured edge clears the cost of taking it. Failing an incomplete floor
/// is still a failure, since the missing part only raises it. Passing one decides nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clearance {
    Clears { gross: i128, floor: i128 },
    DoesNotClear { gross: i128, floor: i128 },
    Undecided { reason: String },
}

impl core::fmt::Display for Clearance {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Clearance::Clears { gross, floor } => {
                write!(f, "clears: {gross} bp/{ONE} gross against a floor of {floor}")
            }
            Clearance::DoesNotClear { gross, floor } => {
                write!(f, "does not clear: {gross} bp/{ONE} gross against a floor of {floor}")
            }
            Clearance::Undecided { reason } => write!(f, "undecided: {reason}"),
        }
    }
}

/// Measured per fold: an average hides whether a result holds in every test window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldMeasure {
    pub fold: usize,
    pub test: Positions,
    pub observations: u64,
    pub spread: i128,
    pub gross: Option<i128>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    HeldOut,
    /// Every observation, because no structure was declared. Promotion refuses on it (`Refusal::InSampleOnly`).
    Everything,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measurement {
    pub run: Digest,
    pub placed: Vec<Placed>,
    pub unplaced: Vec<Unplaced>,
    pub observations: u64,
    pub measured: Vec<Measured>,
    pub dispersion: i128,
    pub floor: Floor,
    pub horizon: Horizon,
    pub clearance: Clearance,
    pub folds: Vec<FoldMeasure>,
    pub basis: Basis,
}

#[derive(Debug)]
pub enum MeasureError {
    Store(String),
    NoSuchRun,
}

impl core::fmt::Display for MeasureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MeasureError::Store(e) => write!(f, "the record could not be read: {e}"),
            MeasureError::NoSuchRun => {
                write!(f, "no entry for that run, so this node cannot say it ran")
            }
        }
    }
}

impl std::error::Error for MeasureError {}

/// Market stream and position come from the run's own admissions, never a caller, so proposals are
/// not priced against a book the strategy was never sent.
pub fn measure(root: &Path, run: Digest) -> Result<Measurement, MeasureError> {
    let held = Recovered::<Ledger>::rebuild(root, RUNS)
        .map_err(|e| MeasureError::Store(format!("{e:?}")))?;
    let ledger =
        held.state().ok_or_else(|| MeasureError::Store("the rebuild did not complete".into()))?;

    if ledger.of(&run).is_empty() {
        return Err(MeasureError::NoSuchRun);
    }

    // From the claim, never from a caller: another horizon would answer an unregistered question.
    let claim =
        standing(root, REGISTRATIONS, run).map_err(|e| MeasureError::Store(format!("{e:?}")))?;
    let horizon = claim.as_ref().map_or(Horizon::NotDeclared, |declared| declared.horizon);
    let folds: Vec<Fold> = claim.map(|declared| declared.folds).unwrap_or_default();

    let mut placed = Vec::new();
    let mut unplaced = Vec::new();
    let mut fees = None;
    let mut venue: Option<String> = None;
    for admission in ledger.admissions_of(&run) {
        // The stream names its venue and the fees are a store declaration. The agent environment
        // may not link anything that fetches one.
        if venue.is_none() {
            venue = venue_of(&admission.market).map(str::to_string);
            fees = venue_fees(root, &admission.market)?;
        }
        price(root, admission, horizon, &mut placed, &mut unplaced)?;
    }

    Ok(summarise(run, horizon, &folds, placed, unplaced, fees, venue))
}

fn summarise(
    run: Digest,
    horizon: Horizon,
    declared: &[Fold],
    placed: Vec<Placed>,
    unplaced: Vec<Unplaced>,
    fees: Option<Fees>,
    venue: Option<String>,
) -> Measurement {
    let all: Vec<&Placed> = placed.iter().filter(|p| p.marketable).collect();

    let folds = fold_measures(declared, &all);

    // Where no structure was declared there is nothing held out. Promotion refuses on that, not here.
    let (basis, crossing): (Basis, Vec<&Placed>) = if declared.is_empty() {
        (Basis::Everything, all)
    } else {
        (
            Basis::HeldOut,
            all.into_iter().filter(|p| declared.iter().any(|f| within(p, f.test))).collect(),
        )
    };
    let spreads: Vec<i128> = crossing.iter().map(|p| p.spread).collect();
    let observations = spreads.len() as u64;

    let mean = mean_of(&spreads);
    let dispersion = deviation_of(&spreads, mean);
    let spread_round_trip = mean.saturating_mul(2);

    // Only proposals that crossed and were marked: averaging in an unmarked one answers a shorter horizon.
    let returns: Vec<i128> = crossing.iter().filter_map(|p| p.gross).collect();
    let gross = (!returns.is_empty()).then(|| floor_mean_of(&returns));

    let mut measured = if observations == 0 {
        Vec::new()
    } else {
        vec![
            Measured { measure: "spread_bps".to_string(), value: mean },
            Measured { measure: "cost_floor_bps".to_string(), value: spread_round_trip },
        ]
    };
    if let Some(gross) = gross {
        measured.push(Measured { measure: "gross_return_bps".to_string(), value: gross });
        measured.push(Measured {
            measure: "net_after_spread_bps".to_string(),
            value: gross.saturating_sub(spread_round_trip),
        });
    }

    let floor = floor_of(fees.as_ref(), venue.as_deref(), spread_round_trip);
    let clearance = clearance_of(gross, &floor, horizon, observations);

    Measurement {
        run,
        placed,
        unplaced,
        observations,
        measured,
        dispersion,
        floor,
        horizon,
        clearance,
        folds,
        basis,
    }
}

/// Observations, mean spread and mean gross return inside each declared fold's test range.
fn fold_measures(declared: &[Fold], all: &[&Placed]) -> Vec<FoldMeasure> {
    declared
        .iter()
        .enumerate()
        .map(|(i, fold)| {
            let inside: Vec<&Placed> =
                all.iter().copied().filter(|p| within(p, fold.test)).collect();
            let returns: Vec<i128> = inside.iter().filter_map(|p| p.gross).collect();
            FoldMeasure {
                fold: i,
                test: fold.test,
                observations: inside.len() as u64,
                spread: mean_of(&inside.iter().map(|p| p.spread).collect::<Vec<_>>()),
                gross: (!returns.is_empty()).then(|| floor_mean_of(&returns)),
            }
        })
        .collect()
}

/// Spread round trip plus the taker fee twice. Only crossing orders are observations, so a
/// resting limit pays no maker fee here. Without a declared fee the floor is unavailable, with
/// the reason.
fn floor_of(fees: Option<&Fees>, venue: Option<&str>, spread_round_trip: i128) -> Floor {
    match fees {
        Some(fees) => Floor::Measured {
            round_trip: spread_round_trip.saturating_add(fees.taker.saturating_mul(2)),
        },
        None => Floor::Unavailable {
            spread_round_trip,
            reason: match venue {
                Some(venue) => format!(
                    "no fee is declared for {venue}, and a verified venue check is where one comes from"
                ),
                None => "this run's market stream names no venue, so no fee can be found for it"
                    .to_string(),
            },
        },
    }
}

/// Tested on the market position it was priced against, not its proposal stream position.
fn within(placed: &Placed, test: Positions) -> bool {
    placed.market_position >= test.from && placed.market_position < test.until
}

fn clearance_of(
    gross: Option<i128>,
    floor: &Floor,
    horizon: Horizon,
    observations: u64,
) -> Clearance {
    if observations == 0 {
        return Clearance::Undecided { reason: "nothing crossed the touch to measure".to_string() };
    }
    let Some(gross) = gross else {
        return Clearance::Undecided {
            reason: match horizon {
                Horizon::NotDeclared => {
                    "the claim declares no horizon, so there is no return to compare".to_string()
                }
                Horizon::NotHeld => {
                    "the claim holds no position, so there is no return to compare".to_string()
                }
                Horizon::After { positions } => {
                    format!("no proposal could be marked {positions} position(s) on")
                }
            },
        };
    };

    // A floor missing a component can only grow, so failing it is an answer and clearing it is not.
    match floor {
        Floor::Measured { round_trip } if gross > *round_trip => {
            Clearance::Clears { gross, floor: *round_trip }
        }
        Floor::Measured { round_trip } => Clearance::DoesNotClear { gross, floor: *round_trip },
        Floor::Unavailable { spread_round_trip, .. } if gross <= *spread_round_trip => {
            Clearance::DoesNotClear { gross, floor: *spread_round_trip }
        }
        Floor::Unavailable { spread_round_trip, reason } => Clearance::Undecided {
            reason: format!(
                "{gross} clears the {spread_round_trip} that was measured, but {reason}"
            ),
        },
    }
}
