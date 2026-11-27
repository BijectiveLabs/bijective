# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""Acceptance tests for the attribution calculator.
Time is supplied as data and all money is Decimal, so results are byte-identical across runs.
"""

from decimal import Decimal

from bijective.attribution.calculator import (
    Availability,
    BookSnapshot,
    CapabilityDeclaration,
    DecisionRecord,
    Fill,
    Instant,
    InsufficientEvidence,
    LiquiditySide,
    Resolution,
    VenueReference,
    attribute,
)

# Nanoseconds, the unit the capture path records in.
NS_PER_MS = 1_000_000

T0_NS = 1_800_000_000_000_000_000


def _decision(*, book: bool = True, decided_ns: int = T0_NS) -> DecisionRecord:
    """Build a decision at a fixed instant, with a book of known shape."""
    snapshot = None
    if book:
        snapshot = BookSnapshot(
            instrument="BTC-PERP",
            best_bid=Decimal("99.90"),
            best_bid_qty=Decimal("50"),
            best_ask=Decimal("100.10"),
            best_ask_qty=Decimal("50"),
            mid=Decimal("100.00"),
            observed_at=Instant(decided_ns, Resolution.NANOSECOND, "A-1"),
        )
    return DecisionRecord(
        decision_id="D-1",
        correlation_id="X-1",
        strategy_id="S-1",
        instrument="BTC-PERP",
        decided_at=Instant(decided_ns, Resolution.NANOSECOND, "A-1"),
        book=snapshot,
    )


def _taker_fill(*, resolution: Resolution = Resolution.NANOSECOND,
                ack_offset_ms: int = 40) -> Fill:
    """A single aggressive buy that lifts the offer."""
    return Fill(
        fill_id="F-1",
        correlation_id="X-1",
        instrument="BTC-PERP",
        side="BUY",
        last_qty=Decimal("10"),
        last_px=Decimal("100.10"),
        fee=Decimal("0.20"),
        liquidity=LiquiditySide.TAKER,
        ts_event=Instant(T0_NS + ack_offset_ms * NS_PER_MS, resolution, "A-1"),
    )


def _all_observed() -> CapabilityDeclaration:
    """A substrate adapter that observes every field this gate set needs."""
    return CapabilityDeclaration(
        liquidity=Availability.OBSERVED,
        acknowledgement=Availability.OBSERVED,
        fee=Availability.OBSERVED,
    )


def _reference() -> VenueReference:
    """Venue reference data with no schedule surprises."""
    return VenueReference(venue="A-1-VENUE", fee_schedule={})


def test_determinism_identical_inputs_yield_identical_output():
    """The same inputs must produce the same result, including the remainder."""
    decision, fills, reference = _decision(), [_taker_fill()], _reference()

    first = attribute(decision, fills, reference, _all_observed())
    second = attribute(decision, fills, reference, _all_observed())

    assert first == second
    assert first.components.unexplained == second.components.unexplained
    assert first.inputs_resolution == second.inputs_resolution


def test_missing_benchmark_refuses_rather_than_estimates():
    """With no decision-time book there is no benchmark, so there is no figure."""
    decision = _decision(book=False)

    try:
        attribute(decision, [_taker_fill()], _reference(), _all_observed())
    except InsufficientEvidence as refusal:
        assert refusal.reason
        assert "benchmark" in " ".join(refusal.missing).lower()
    else:
        raise AssertionError(
            "attribution produced a figure without a decision-time benchmark"
        )


def test_unavailable_liquidity_is_not_assumed_to_be_taker():
    """An unobserved maker/taker side must stay unavailable, never assumed."""
    declaration = CapabilityDeclaration(
        liquidity=Availability.UNAVAILABLE,
        acknowledgement=Availability.OBSERVED,
        fee=Availability.OBSERVED,
    )

    result = attribute(_decision(), [_taker_fill()], _reference(), declaration)

    assert result.components.fee.availability is Availability.UNAVAILABLE
    assert result.components.fee.value is None
    assert result.components.unexplained == Decimal("0.20")


def test_book_observed_after_the_decision_is_rejected():
    """A benchmark may not be built from information that did not yet exist."""
    decision = _decision()
    decision.book.observed_at = Instant(T0_NS + NS_PER_MS, Resolution.NANOSECOND, "A-1")

    try:
        attribute(decision, [_taker_fill()], _reference(), _all_observed())
    except InsufficientEvidence as refusal:
        assert "look-ahead" in refusal.reason.lower()
    else:
        raise AssertionError("a benchmark was built from a post-decision observation")


def test_book_observed_exactly_at_the_decision_is_admissible():
    """The boundary is strictly after the decision, not at it."""
    result = attribute(_decision(), [_taker_fill()], _reference(), _all_observed())
    assert result.benchmark_px == Decimal("100.00")


def test_known_cost_decomposes_to_the_constructed_values():
    """The only gate that proves the arithmetic rather than the refusal machinery."""
    result = attribute(_decision(), [_taker_fill()], _reference(), _all_observed())

    assert result.benchmark_px == Decimal("100.00")
    assert result.realised_px == Decimal("100.10")
    assert result.shortfall == Decimal("1.20")
    assert result.components.crossing.value == Decimal("1.00")
    assert result.components.fee.value == Decimal("0.20")
    assert result.components.ack_residual.value == Decimal("0.00")
    assert result.components.unexplained == Decimal("0.00")


def test_second_grade_input_is_not_reported_at_finer_resolution():
    """A figure may not be finer than the record it came from."""
    fill = _taker_fill(resolution=Resolution.SECOND)

    result = attribute(_decision(), [fill], _reference(), _all_observed())

    assert result.inputs_resolution is Resolution.SECOND
    assert result.components.ack_residual.availability is Availability.UNAVAILABLE
    assert result.components.ack_residual.value is None
    assert "resolution" in result.components.ack_residual.reason.lower()


def test_unrecognised_side_is_refused_not_treated_as_a_sell():
    """Direction decides which side of the book the order had to reach."""
    fill = _taker_fill()
    fill.side = "LONG"  # not a side this component recognises

    try:
        attribute(_decision(), [fill], _reference(), _all_observed())
    except InsufficientEvidence as refusal:
        assert "side" in " ".join(refusal.missing).lower()
    else:
        raise AssertionError("an unrecognised side was interpreted rather than refused")


def test_opposing_sides_under_one_decision_are_refused():
    """Two directions cannot share one benchmark."""
    buy = _taker_fill()
    sell = _taker_fill()
    sell.fill_id = "F-2"
    sell.side = "SELL"

    try:
        attribute(_decision(), [buy, sell], _reference(), _all_observed())
    except InsufficientEvidence as refusal:
        assert "side" in " ".join(refusal.missing).lower()
    else:
        raise AssertionError("opposing sides were averaged against one benchmark")
