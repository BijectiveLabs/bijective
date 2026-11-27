# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""Acceptance tests for the fill correlator.
Every fill leaves as a correlated record or a counted exclusion. Time is data; no clock, no network.
"""

from decimal import Decimal

from bijective.attribution.calculator import (
    BookSnapshot,
    DecisionRecord,
    Fill,
    Instant,
    LiquiditySide,
    Resolution,
)
from bijective.attribution.correlator import Order, correlate

NS_PER_MS = 1_000_000
T0_NS = 1_800_000_000_000_000_000


def _fill(fill_id: str, correlation_id: str) -> Fill:
    """A minimal well-formed execution."""
    return Fill(
        fill_id=fill_id,
        correlation_id=correlation_id,
        instrument="BTC-PERP",
        side="BUY",
        last_qty=Decimal("1"),
        last_px=Decimal("100.10"),
        fee=Decimal("0.02"),
        liquidity=LiquiditySide.TAKER,
        ts_event=Instant(T0_NS + NS_PER_MS, Resolution.NANOSECOND, "A-1"),
    )


def _decision(correlation_id: str) -> DecisionRecord:
    """A decision carrying a book, so a correlated pair is usable downstream."""
    book = BookSnapshot(
        instrument="BTC-PERP",
        best_bid=Decimal("99.90"),
        best_bid_qty=Decimal("50"),
        best_ask=Decimal("100.10"),
        best_ask_qty=Decimal("50"),
        mid=Decimal("100.00"),
        observed_at=Instant(T0_NS, Resolution.NANOSECOND, "A-1"),
    )
    return DecisionRecord(
        decision_id=f"D-{correlation_id}",
        correlation_id=correlation_id,
        strategy_id="S-1",
        instrument="BTC-PERP",
        decided_at=Instant(T0_NS, Resolution.NANOSECOND, "A-1"),
        book=book,
    )


def test_every_fill_is_either_correlated_or_counted():
    """One hundred fills in, of which seven cannot be tied to an order."""
    correlated_ids = [f"X-{i}" for i in range(93)]
    orders = [Order(client_order_id=f"O-{i}", correlation_id=x)
              for i, x in enumerate(correlated_ids)]
    decisions = [_decision(x) for x in correlated_ids]

    fills = [_fill(f"F-{i}", x) for i, x in enumerate(correlated_ids)]
    fills.extend(_fill(f"F-orphan-{i}", "") for i in range(7))

    records, ledger = correlate(fills, orders, decisions)

    assert ledger.population_total == 100
    assert ledger.uncorrelated_count == 7
    assert len(records) == 93
    assert len(records) + ledger.uncorrelated_count == ledger.population_total
    assert sum(ledger.reasons.values()) == 7

    coverage = Decimal(len(records)) / Decimal(ledger.population_total)
    assert coverage == Decimal("0.93")


def test_an_unknown_order_reference_is_excluded_with_its_own_reason():
    """A reference that is present but resolves to nothing is still an exclusion."""
    fills = [_fill("F-1", "X-unknown")]

    records, ledger = correlate(fills, [], [])

    assert records == []
    assert ledger.uncorrelated_count == 1
    assert len(ledger.reasons) == 1
    assert sum(ledger.reasons.values()) == 1


def test_a_correlated_fill_without_a_decision_is_not_an_exclusion():
    """Not measurable and not accounted for are different things."""
    orders = [Order(client_order_id="O-1", correlation_id="X-1")]
    fills = [_fill("F-1", "X-1")]

    records, ledger = correlate(fills, orders, decisions=[])

    assert ledger.uncorrelated_count == 0
    assert len(records) == 1
    assert records[0].decision is None
    assert records[0].fill.fill_id == "F-1"


def test_output_order_is_stable_for_identical_input():
    """Downstream determinism depends on this component not reordering its output."""
    orders = [Order(client_order_id=f"O-{i}", correlation_id=f"X-{i}")
              for i in range(5)]
    fills = [_fill(f"F-{i}", f"X-{i}") for i in range(5)]

    first, _ = correlate(fills, orders, [])
    second, _ = correlate(fills, orders, [])

    assert [r.fill.fill_id for r in first] == [r.fill.fill_id for r in second]
    assert [r.fill.fill_id for r in first] == [f"F-{i}" for i in range(5)]
