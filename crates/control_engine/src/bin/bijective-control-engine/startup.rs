// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! What a node sets up before its first pass: the catalogue, the rates, the limits in force and
//! the venues that did not open.

use std::collections::BTreeMap;

use bijective_control_engine::wire::Participation;
use bijective_control_engine::{Accepted, OrderPath};
use bijective_limits::Limit;
use bijective_model::InstrumentId;
use bijective_risk::valuation::currency::Rates;
use bijective_venue::profile::Instrument;

use crate::clock::now_ns;
use crate::venues::{OpenVenues, Withdrawn};
use crate::{Fatal, PROGRAM};

/// Venues trading on a profile no verifier has checked. Reported, not refused, until a verifier
/// runs on every node.
pub(crate) fn unverified_venues(adapters: &OpenVenues) -> Vec<String> {
    let unverified: Vec<String> = adapters
        .iter()
        .filter(|(_, a)| !a.declaration().is_verified())
        .map(|(name, _)| name.clone())
        .collect();
    for venue in &unverified {
        eprintln!(
            "{PROGRAM}: {venue} trades on a profile no verifier has checked against the venue"
        );
    }
    unverified
}

/// One catalogue keyed by platform identity. An identity declared by two venues is refused:
/// merging on a coincidence of names would combine two venues' positions under one key. Accounts
/// at one venue share it when they list the same instrument (decision 0008).
pub(crate) fn catalogue_by_identity(
    adapters: &OpenVenues,
) -> Result<BTreeMap<InstrumentId, Instrument>, Fatal> {
    let mut by_identity: BTreeMap<InstrumentId, (String, Instrument)> = BTreeMap::new();
    for (account, adapter) in adapters.iter() {
        let dialect = adapter.declaration().venue();
        for instrument in adapter.declaration().instruments().iter() {
            match by_identity.get(&instrument.id) {
                Some((held, listed)) if held == dialect && listed == instrument => {}
                Some(_) => {
                    return Err(Fatal::failure(format!(
                        "{account} declares {}, which another venue already declared; a shared \
                         identity has to be declared rather than coincide",
                        instrument.id
                    )));
                }
                None => {
                    by_identity
                        .insert(instrument.id.clone(), (dialect.to_string(), instrument.clone()));
                }
            }
        }
    }
    Ok(by_identity.into_iter().map(|(id, (_, instrument))| (id, instrument)).collect())
}

/// The declared reporting currency, each rate market checked against the traded catalogue and
/// the rate streams' profiles. A market that cannot carry its rate stops the node
/// (`docs/decisions/0005-reporting-currency.md`).
pub(crate) fn rates_of(
    accepted: &Accepted,
    catalogue: &BTreeMap<InstrumentId, Instrument>,
) -> Result<Rates, Fatal> {
    let Some(reporting) = accepted.reporting() else { return Ok(Rates::single().clone()) };
    let mut listed = catalogue.clone();
    for market in &reporting.markets {
        let refused = |why: String| Fatal::failure(format!("rate market {}: {why}", market.name));
        let profile =
            crate::venues::read_profile(&market.profile, accepted.assets()).map_err(refused)?;
        for instrument in profile.instruments.iter() {
            if listed.insert(instrument.id.clone(), instrument.clone()).is_some() {
                return Err(refused(format!("{} is declared twice", instrument.id)));
            }
        }
    }
    Rates::declared(&reporting.currency, &reporting.rates, &listed)
        .map_err(|e| Fatal::failure(format!("the valuation was refused: {e}")))
}

/// Declares each configured limit not already in force, so a restart appends no duplicate.
pub(crate) fn put_limits_in_force(
    path: &mut OrderPath,
    accepted: &Accepted,
) -> Result<(), bijective_risk::limit_state::StateError> {
    for (scope, kind, maximum) in accepted.limits() {
        let (state, writer) = path.limits_mut();
        if !matches!(state.budget(scope, *kind), bijective_risk::limit_state::Budget::Undeclared) {
            continue;
        }
        state.declare(writer, scope.clone(), *kind, *maximum, 1)?;
    }
    Ok(())
}

/// Every configured limit, constructed. One refused limit stops the node: running with fewer
/// limits than declared is the one direction a risk system must never fail in.
pub(crate) fn declare_limits(accepted: &Accepted) -> Result<Vec<Limit>, Fatal> {
    let mut declared = Vec::with_capacity(accepted.limits().len());
    for (scope, kind, maximum) in accepted.limits() {
        let limit = Limit::declare(scope.clone(), *kind, *maximum).map_err(|e| {
            Fatal::failure(format!("the limit declared for {scope:?} was refused: {e:?}"))
        })?;
        declared.push(limit);
    }
    Ok(declared)
}

pub(crate) fn record_withdrawn_at_open(path: &mut OrderPath, withdrawn: &[Withdrawn]) {
    for w in withdrawn {
        let participation = Participation::Withdrawn { reason: w.reason.clone() };
        if let Err(e) = path.record_participation(&w.venue, participation, now_ns()) {
            eprintln!("{PROGRAM}: the withdrawal of {} could not be recorded: {e:?}", w.venue);
        }
    }
}
