// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! Declared limits: scope, kind and maximum, in the unit the kind is judged in.

use std::collections::BTreeMap;

use bijective_common::configuration::{Invalid, Reason, positive, required};
use bijective_limits::{Amount, LimitKind, Ratio, Scope, Threshold, Unit};

use super::{DeskDeclaration, LimitDeclaration, declared_desk, scope, wide};

/// Kinds an operator may declare here. The ratio kinds are judged on each pass, never drawn down.
const DECLARABLE_KINDS: [(LimitKind, &str); 6] = [
    (LimitKind::GrossExposure, "gross_exposure"),
    (LimitKind::NetExposure, "net_exposure"),
    (LimitKind::DailyLoss, "daily_loss"),
    (LimitKind::Drawdown, "drawdown"),
    (LimitKind::Leverage, "leverage"),
    (LimitKind::MarginUse, "margin_use"),
];

fn kind(field: &str, text: &str) -> Result<LimitKind, Invalid> {
    match DECLARABLE_KINDS.iter().find(|(_, spelling)| *spelling == text) {
        Some((kind, _)) => Ok(*kind),
        None => Err(Invalid::new(
            field,
            Reason::Unknown {
                value: text.to_string(),
                known: DECLARABLE_KINDS.iter().map(|(_, s)| (*s).to_string()).collect(),
            },
        )),
    }
}

/// Refuse a set where a declared kind leaves part of the book unbound.
fn coverage(
    limits: &[(Scope, LimitKind, Threshold)],
    desks: &[DeskDeclaration],
) -> Result<(), Invalid> {
    for (declared, spelling) in DECLARABLE_KINDS {
        let of_kind = |scope: &Scope| limits.iter().any(|(s, k, _)| *k == declared && s == scope);

        if !limits.iter().any(|(_, k, _)| *k == declared) || of_kind(&Scope::Portfolio) {
            continue;
        }

        for desk in desks {
            for venue in &desk.venues {
                for instrument in &desk.instruments {
                    if of_kind(&Scope::Desk(desk.name.clone()))
                        || of_kind(&Scope::Venue(venue.clone()))
                        || of_kind(&Scope::Instrument(instrument.clone()))
                    {
                        continue;
                    }
                    return Err(Invalid::new(
                        format!("limits[{spelling}]"),
                        Reason::Requires {
                            other: format!(
                                "portfolio/{spelling}, desk:{}/{spelling}, \
                                 venue:{venue}/{spelling} or instrument:{instrument}/{spelling}",
                                desk.name
                            ),
                        },
                    ));
                }
            }
        }
    }

    Ok(())
}

/// Every declared limit, distinct per scope and kind, with a portfolio limit present and every
/// kind covering every entity the desks declare.
pub(super) fn limits_of(
    declarations: &[LimitDeclaration],
    desks: &[DeskDeclaration],
) -> Result<Vec<(Scope, LimitKind, Threshold)>, Invalid> {
    let mut limits = Vec::new();
    let mut declared: BTreeMap<(String, String), ()> = BTreeMap::new();
    for limit in declarations {
        let field = format!("limits.{}/{}", limit.scope, limit.kind);
        if declared.insert((limit.scope.clone(), limit.kind.clone()), ()).is_some() {
            return Err(Invalid::new(
                "limits",
                Reason::Duplicate { value: format!("{}/{}", limit.scope, limit.kind) },
            ));
        }

        let scope = scope(&format!("{field}.scope"), &limit.scope)?;
        declared_desk(&format!("{field}.scope"), &scope, desks)?;
        let kind = kind(&format!("{field}.kind"), &limit.kind)?;
        let maximum = threshold_of(&field, kind, limit)?;
        // Measured for the whole book only: a loss, drawdown, leverage or margin use by venue or
        // instrument has no figure, so the limit would be unjudged and halt the node every pass.
        let whole_book = matches!(
            kind,
            LimitKind::DailyLoss | LimitKind::Drawdown | LimitKind::Leverage | LimitKind::MarginUse
        );
        if whole_book && scope != Scope::Portfolio {
            return Err(Invalid::new(
                format!("{field}.scope"),
                Reason::Unknown {
                    value: limit.scope.clone(),
                    known: vec!["portfolio".to_string()],
                },
            ));
        }
        limits.push((scope, kind, maximum));
    }

    // A portfolio scope covers every entity by construction. Without one completeness cannot be shown.
    if !limits.iter().any(|(scope, _, _)| *scope == Scope::Portfolio) {
        return Err(Invalid::new("limits[portfolio]", Reason::Absent));
    }

    // Every declared kind must reach every entity. One portfolio limit of another kind does not.
    coverage(&limits, desks)?;

    Ok(limits)
}

/// The maximum in the unit the kind is judged in: money at the money scale, a ratio at
/// `Ratio::ONE`. The other unit's field is refused, not ignored: it would read as a bound in force.
fn threshold_of(
    field: &str,
    kind: LimitKind,
    limit: &LimitDeclaration,
) -> Result<Threshold, Invalid> {
    let money = format!("{field}.maximum_money_scaled");
    let ratio = format!("{field}.maximum_ratio_scaled");
    let (named, given, other, stray) = match kind.unit() {
        Unit::Money => (&money, &limit.maximum_money_scaled, &ratio, &limit.maximum_ratio_scaled),
        Unit::Ratio => (&ratio, &limit.maximum_ratio_scaled, &money, &limit.maximum_money_scaled),
    };
    if stray.is_some() {
        return Err(Invalid::new(other.clone(), Reason::Conflicting { with: named.clone() }));
    }
    let maximum = positive(named, wide(named, required(named, given.as_deref())?)?)?;
    Ok(match kind.unit() {
        Unit::Money => Threshold::Money(Amount::from_scaled(maximum)),
        Unit::Ratio => Threshold::Ratio(Ratio::from_scaled(maximum)),
    })
}
