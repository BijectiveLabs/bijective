# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""C2 - the fill correlator. Every fill leaves as a correlated record or a counted exclusion.
A silently dropped fill would shrink the denominator of every figure downstream. Read-side only.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from bijective.attribution.calculator import DecisionRecord, Fill

# Stable identifiers rather than free text, so their mix over time is a signal.
# Kept apart: a malformed producer and a gap in the order record have different fixes.
REASON_MISSING_REFERENCE = "missing_order_reference"
REASON_UNKNOWN_REFERENCE = "unknown_order_reference"


@dataclass(frozen=True)
class Order:
    """Links an execution to its decision. The venue echoes client_order_id, not our correlation_id."""

    client_order_id: str
    correlation_id: str


@dataclass
class CorrelatedFill:
    """An execution with its decision, where one was captured. A None decision is valid: no benchmark, no fault."""

    fill: Fill
    decision: DecisionRecord | None


@dataclass
class ExclusionLedger:
    """What did not correlate, and why. `population_total` lets a roll-up state the share of fills it covers."""

    population_total: int
    uncorrelated_count: int
    reasons: dict[str, int] = field(default_factory=dict)


def correlate(
    fills: list[Fill],
    orders: list[Order],
    decisions: list[DecisionRecord],
) -> tuple[list[CorrelatedFill], ExclusionLedger]:
    """Correlate executions to their orders and decisions.
    Input order is preserved; correlated and excluded counts always sum to the population.
    """
    known_correlations = {o.correlation_id for o in orders if o.correlation_id}
    decisions_by_correlation = {
        d.correlation_id: d for d in decisions if d.correlation_id
    }
    # Mirrors the substrate's own secondary index on client order id rather than adding a second matcher.
    correlation_by_client_order = {
        o.client_order_id: o.correlation_id
        for o in orders
        if o.client_order_id and o.correlation_id
    }

    records: list[CorrelatedFill] = []
    reasons: dict[str, int] = {}

    for fill in fills:
        # The correlation id is ours. The venue echoes only the client order id. A fill carrying only
        # the echo is resolvable and must not be excluded.
        reference = (fill.correlation_id or "").strip()
        echoed = (fill.client_order_id or "").strip()

        if not reference and not echoed:
            # Nothing to tie it to: a producer-side defect, counted so the gap is visible.
            reasons[REASON_MISSING_REFERENCE] = (
                reasons.get(REASON_MISSING_REFERENCE, 0) + 1
            )
            continue

        if reference not in known_correlations:
            # Our identifier takes precedence. The venue's echo is a third party's report of it.
            resolved = correlation_by_client_order.get(echoed, "") if echoed else ""
            if not resolved:
                # A gap in the order record, not a malformed execution. Kept as a distinct reason.
                reasons[REASON_UNKNOWN_REFERENCE] = (
                    reasons.get(REASON_UNKNOWN_REFERENCE, 0) + 1
                )
                continue
            reference = resolved

        # A missing decision is not an exclusion: the fill is real, just not benchmarkable.
        records.append(
            CorrelatedFill(
                fill=fill, decision=decisions_by_correlation.get(reference)
            )
        )

    ledger = ExclusionLedger(
        population_total=len(fills),
        uncorrelated_count=sum(reasons.values()),
        reasons=reasons,
    )

    # Asserted: if it fails, every downstream figure is over an unknown population.
    if len(records) + ledger.uncorrelated_count != ledger.population_total:
        raise AssertionError(
            "correlation lost a fill: correlated and excluded counts do not sum "
            "to the population that entered"
        )

    return records, ledger
