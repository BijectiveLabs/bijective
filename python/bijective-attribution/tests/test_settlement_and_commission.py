# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""Acceptance gates for settlement state and commission currency in C3.
Both are venue behaviours where the component would otherwise print a confident wrong figure.
"""

from decimal import Decimal

import pytest

from bijective.attribution.calculator import (
    Availability,
    BookSnapshot,
    CapabilityDeclaration,
    DecisionRecord,
    Fill,
    Instant,
    LiquiditySide,
    Resolution,
    SettlementState,
    VenueReference,
    attribute,
)

NS_PER_MS = 1_000_000
T0_NS = 1_800_000_000_000_000_000


def _decision() -> DecisionRecord:
    """A decision with a book of known shape: mid 100.00, one tick either side."""
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
        decision_id="D-1",
        correlation_id="X-1",
        strategy_id="S-1",
        instrument="BTC-PERP",
        decided_at=Instant(T0_NS, Resolution.NANOSECOND, "A-1"),
        book=book,
    )


def _fill(
    *,
    settlement: SettlementState = SettlementState.PROVISIONAL,
    fee_currency: str = "USDT",
) -> Fill:
    """A taker buy of ten units at the touch, fee 0.02 per unit."""
    return Fill(
        fill_id="F-1",
        correlation_id="X-1",
        instrument="BTC-PERP",
        side="BUY",
        last_qty=Decimal("10"),
        last_px=Decimal("100.10"),
        fee=Decimal("0.20"),
        fee_currency=fee_currency,
        liquidity=LiquiditySide.TAKER,
        ts_event=Instant(T0_NS + 40 * NS_PER_MS, Resolution.NANOSECOND, "A-1"),
        settlement=settlement,
    )


def _all_observed() -> CapabilityDeclaration:
    return CapabilityDeclaration(
        liquidity=Availability.OBSERVED,
        acknowledgement=Availability.OBSERVED,
        fee=Availability.OBSERVED,
    )


def _reference(cost_currency: str = "USDT") -> VenueReference:
    """Venue reference carrying the instrument's cost currency."""
    return VenueReference(venue="A-1-VENUE", fee_schedule={}, cost_currency=cost_currency)


def test_an_attribution_over_an_unsettled_fill_is_marked_provisional():
    """While a venue can still correct the fill, the figure is not final."""
    result = attribute(_decision(), [_fill()], _reference(), _all_observed())

    assert result.settlement is SettlementState.PROVISIONAL
    assert result.is_final is False


def test_an_attribution_over_a_settled_fill_is_final():
    """Once the venue can no longer correct it, the figure is final."""
    result = attribute(
        _decision(),
        [_fill(settlement=SettlementState.SETTLED)],
        _reference(),
        _all_observed(),
    )

    assert result.settlement is SettlementState.SETTLED
    assert result.is_final is True


def test_a_voided_fill_is_refused_not_attributed():
    """A voided fill did not happen; attributing it would invent a cost."""
    from bijective.attribution.calculator import InsufficientEvidence

    try:
        attribute(
            _decision(),
            [_fill(settlement=SettlementState.VOIDED)],
            _reference(),
            _all_observed(),
        )
    except InsufficientEvidence as refusal:
        assert "void" in refusal.reason.lower()
    else:
        raise AssertionError("a voided fill was attributed instead of refused")


def test_a_mixed_settlement_group_is_provisional():
    """One unsettled member makes the whole attribution provisional."""
    settled = _fill(settlement=SettlementState.SETTLED)
    provisional = _fill(settlement=SettlementState.PROVISIONAL)
    provisional.fill_id = "F-2"

    result = attribute(
        _decision(), [settled, provisional], _reference(), _all_observed()
    )

    assert result.settlement is SettlementState.PROVISIONAL
    assert result.is_final is False


def test_fee_in_the_cost_currency_is_marked_as_counted_by_the_substrate():
    """A fee in the position's cost currency is one the substrate's PnL includes."""
    result = attribute(_decision(), [_fill()], _reference(), _all_observed())

    assert result.components.fee.value == Decimal("0.20")
    assert result.fee_currency == "USDT"
    assert result.fee_counted_in_substrate_pnl is True


def test_fee_outside_the_cost_currency_is_flagged_as_uncounted():
    """A fee in another currency never reaches the substrate's realized PnL."""
    fill = _fill(fee_currency="BNB")

    result = attribute(_decision(), [fill], _reference(cost_currency="USDT"), _all_observed())

    assert result.fee_currency == "BNB"
    assert result.fee_counted_in_substrate_pnl is False
    assert result.components.fee.value == Decimal("0.20")


def test_mixed_fee_currencies_in_one_group_are_refused():
    """Fees in different currencies cannot be summed into one figure."""
    from bijective.attribution.calculator import InsufficientEvidence

    usdt = _fill(fee_currency="USDT")
    bnb = _fill(fee_currency="BNB")
    bnb.fill_id = "F-2"

    try:
        attribute(_decision(), [usdt, bnb], _reference(), _all_observed())
    except InsufficientEvidence as refusal:
        assert "currency" in " ".join(refusal.missing).lower()
    else:
        raise AssertionError("fees in differing currencies were summed")


@pytest.mark.parametrize("cost_currency", ["USDT", "BTC"])
def test_the_cost_currency_comes_from_venue_reference_not_assumption(cost_currency):
    """Cost currency is quote, base or settlement depending on the contract."""
    fill = _fill(fee_currency=cost_currency)

    result = attribute(
        _decision(), [fill], _reference(cost_currency=cost_currency), _all_observed()
    )

    assert result.fee_counted_in_substrate_pnl is True
