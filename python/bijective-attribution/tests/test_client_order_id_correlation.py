# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""Acceptance tests for correlating fills on the venue-echoed client order id.
A fill is excluded only when no identifier it carries resolves.
"""

from decimal import Decimal

from bijective.attribution.calculator import (
    Fill,
    Instant,
    LiquiditySide,
    Resolution,
)
from bijective.attribution.correlator import (
    REASON_MISSING_REFERENCE,
    REASON_UNKNOWN_REFERENCE,
    Order,
    correlate,
)

T0_NS = 1_800_000_000_000_000_000


def _fill(*, fill_id: str, correlation_id: str = "", client_order_id: str = "") -> Fill:
    """A fill carrying whichever identifiers the test is exercising."""
    return Fill(
        fill_id=fill_id,
        correlation_id=correlation_id,
        instrument="BTC-PERP",
        side="BUY",
        last_qty=Decimal("1"),
        last_px=Decimal("100.00"),
        fee=Decimal("0.02"),
        fee_currency="USDT",
        liquidity=LiquiditySide.TAKER,
        ts_event=Instant(T0_NS, Resolution.NANOSECOND, "A-1"),
        client_order_id=client_order_id,
    )


def test_a_fill_carrying_only_a_client_order_id_correlates():
    """The identifier the venue echoes is sufficient to correlate."""
    fill = _fill(fill_id="F-1", client_order_id="O-1")
    orders = [Order(client_order_id="O-1", correlation_id="X-1")]

    records, ledger = correlate([fill], orders, [])

    assert ledger.uncorrelated_count == 0
    assert len(records) == 1
    assert records[0].fill.fill_id == "F-1"


def test_the_correlation_id_is_preferred_when_both_are_present():
    """Where the platform's own identifier is present it is authoritative."""
    fill = _fill(fill_id="F-1", correlation_id="X-1", client_order_id="O-2")
    orders = [
        Order(client_order_id="O-1", correlation_id="X-1"),
        Order(client_order_id="O-2", correlation_id="X-2"),
    ]

    records, ledger = correlate([fill], orders, [])

    assert ledger.uncorrelated_count == 0
    assert len(records) == 1


def test_a_client_order_id_matching_no_order_is_an_unknown_reference():
    """An identifier that resolves to nothing is a gap in the order record."""
    fill = _fill(fill_id="F-1", client_order_id="O-UNKNOWN")

    records, ledger = correlate([fill], [Order("O-1", "X-1")], [])

    assert not records
    assert ledger.reasons.get(REASON_UNKNOWN_REFERENCE) == 1


def test_a_fill_carrying_neither_identifier_is_a_missing_reference():
    """Nothing to resolve is a producer-side defect, and stays counted as one."""
    fill = _fill(fill_id="F-1")

    records, ledger = correlate([fill], [Order("O-1", "X-1")], [])

    assert not records
    assert ledger.reasons.get(REASON_MISSING_REFERENCE) == 1


def test_the_population_invariant_holds_across_both_identifier_paths():
    """Correlated and excluded still sum to what entered, whichever path resolved them."""
    fills = [
        _fill(fill_id="F-1", correlation_id="X-1"),
        _fill(fill_id="F-2", client_order_id="O-2"),
        _fill(fill_id="F-3", client_order_id="O-MISSING"),
        _fill(fill_id="F-4"),
    ]
    orders = [Order("O-1", "X-1"), Order("O-2", "X-2")]

    records, ledger = correlate(fills, orders, [])

    assert ledger.population_total == 4
    assert len(records) == 2
    assert ledger.uncorrelated_count == 2
    assert len(records) + ledger.uncorrelated_count == ledger.population_total


def test_a_decision_is_found_through_the_client_order_id_path():
    """Resolution must yield the correlation id itself, not merely a pass or fail."""
    from bijective.attribution.calculator import BookSnapshot, DecisionRecord

    book = BookSnapshot(
        instrument="BTC-PERP",
        best_bid=Decimal("99.90"),
        best_bid_qty=Decimal("50"),
        best_ask=Decimal("100.10"),
        best_ask_qty=Decimal("50"),
        mid=Decimal("100.00"),
        observed_at=Instant(T0_NS, Resolution.NANOSECOND, "A-1"),
    )
    decision = DecisionRecord(
        decision_id="D-1",
        correlation_id="X-1",
        strategy_id="S-1",
        instrument="BTC-PERP",
        decided_at=Instant(T0_NS, Resolution.NANOSECOND, "A-1"),
        book=book,
    )

    records, _ = correlate(
        [_fill(fill_id="F-1", client_order_id="O-1")],
        [Order(client_order_id="O-1", correlation_id="X-1")],
        [decision],
    )

    assert len(records) == 1
    assert records[0].decision is not None
    assert records[0].decision.decision_id == "D-1"
