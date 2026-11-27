# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""Acceptance gates for C4, the attribution store and roll-up.
Covers coverage and no silent widening, and recovery of an interrupted store.
"""

from decimal import Decimal

from bijective.attribution.calculator import (
    Attribution,
    Availability,
    CostComponents,
    Measured,
    Resolution,
)
from bijective.attribution.store import (
    Dimension,
    InMemoryRollUpStore,
    RollUpKey,
    RollUpService,
    StoreNotVerified,
)


def _measured_money(value: str, availability: Availability = Availability.OBSERVED) -> Measured:
    """A money measurement, observed by default."""
    if availability is Availability.UNAVAILABLE:
        return Measured(
            value=None, availability=availability, reason="unavailable for this fixture"
        )
    return Measured(
        value=Decimal(value), availability=availability, resolution=Resolution.NANOSECOND
    )


def _attribution(*, decision_id: str, shortfall: str, fee_available: bool = True) -> Attribution:
    """A complete attribution with a hand-set shortfall."""
    fee = _measured_money(
        "0.20", Availability.OBSERVED if fee_available else Availability.UNAVAILABLE
    )
    return Attribution(
        decision_id=decision_id,
        instrument="BTC-PERP",
        filled_qty=Decimal("10"),
        benchmark_px=Decimal("100.00"),
        realised_px=Decimal("100.10"),
        shortfall=Decimal(shortfall),
        components=CostComponents(
            crossing=_measured_money("1.00"),
            fee=fee,
            ack_residual=_measured_money("0.00"),
            unexplained=Decimal("0.00"),
        ),
        inputs_resolution=Resolution.NANOSECOND,
        computed_from=(f"F-{decision_id}",),
    )


def _key(*, strategy: str = "S-1", instrument: str = "BTC-PERP",
         venue: str = "V-1", period: str = "2026-07") -> RollUpKey:
    return RollUpKey(
        strategy_id=strategy, instrument=instrument, venue=venue, period=period
    )


def test_rollup_sums_shortfall_over_the_group():
    """Shortfall is additive, so the group total is the sum of its members."""
    service = RollUpService(InMemoryRollUpStore())
    service.record(_attribution(decision_id="1", shortfall="1.20"), _key())
    service.record(_attribution(decision_id="2", shortfall="1.20"), _key())

    rollup = service.rollup(Dimension.STRATEGY, "S-1")

    assert rollup.shortfall_total == Decimal("2.40")
    assert rollup.population_computed == 2


def test_a_component_total_is_unavailable_if_any_member_is_unavailable():
    """No silent widening at the aggregate level."""
    service = RollUpService(InMemoryRollUpStore())
    service.record(_attribution(decision_id="1", shortfall="1.20"), _key())
    service.record(
        _attribution(decision_id="2", shortfall="1.00", fee_available=False), _key()
    )

    rollup = service.rollup(Dimension.STRATEGY, "S-1")

    assert rollup.components.fee.availability is Availability.UNAVAILABLE
    assert rollup.components.fee.value is None
    assert rollup.components.fee.reason
    assert rollup.components.crossing.value == Decimal("2.00")


def test_coverage_counts_refusals_in_the_denominator():
    """Coverage is computed over the population, not over what happened to compute."""
    service = RollUpService(InMemoryRollUpStore())
    service.record(_attribution(decision_id="1", shortfall="1.20"), _key())
    service.record(_attribution(decision_id="2", shortfall="1.20"), _key())
    service.record(_attribution(decision_id="3", shortfall="1.20"), _key())
    service.record_refusal(_key(), "no decision-time book")

    rollup = service.rollup(Dimension.STRATEGY, "S-1")

    assert rollup.population_computed == 3
    assert rollup.population_total == 4
    assert rollup.coverage == Decimal("0.75")


def test_every_rollup_carries_coverage():
    """The contract invariant: an aggregate without coverage is not admissible."""
    service = RollUpService(InMemoryRollUpStore())
    service.record(_attribution(decision_id="1", shortfall="1.20"), _key())

    rollup = service.rollup(Dimension.STRATEGY, "S-1")

    assert rollup.coverage is not None
    assert rollup.coverage == Decimal("1")


def test_rollup_is_deterministic_for_identical_records():
    """Recording the same set in a different order yields the same aggregate."""
    forward = RollUpService(InMemoryRollUpStore())
    reverse = RollUpService(InMemoryRollUpStore())
    records = [_attribution(decision_id=str(i), shortfall="1.20") for i in range(5)]
    for r in records:
        forward.record(r, _key())
    for r in reversed(records):
        reverse.record(r, _key())

    a = forward.rollup(Dimension.STRATEGY, "S-1")
    b = reverse.rollup(Dimension.STRATEGY, "S-1")

    assert a.shortfall_total == b.shortfall_total
    assert a.population_total == b.population_total
    assert a.coverage == b.coverage


def test_rollup_by_instrument_isolates_the_group():
    """A roll-up over one dimension value does not leak records from another."""
    service = RollUpService(InMemoryRollUpStore())
    service.record(
        _attribution(decision_id="1", shortfall="1.20"), _key(instrument="BTC-PERP")
    )
    service.record(
        _attribution(decision_id="2", shortfall="5.00"), _key(instrument="ETH-PERP")
    )

    btc = service.rollup(Dimension.INSTRUMENT, "BTC-PERP")

    assert btc.shortfall_total == Decimal("1.20")
    assert btc.population_computed == 1


def test_a_group_with_only_refusals_reports_no_cost_not_zero_cost():
    """Found in the component's own review, not from the numbered gates."""
    service = RollUpService(InMemoryRollUpStore())
    service.record_refusal(_key(), "no decision-time book")
    service.record_refusal(_key(), "no decision-time book")

    rollup = service.rollup(Dimension.STRATEGY, "S-1")

    assert rollup.population_computed == 0
    assert rollup.population_total == 2
    assert rollup.coverage == Decimal("0")
    assert rollup.components.crossing.availability is Availability.UNAVAILABLE
    assert rollup.components.fee.availability is Availability.UNAVAILABLE


def test_an_unverified_store_serves_nothing():
    """After an interrupted write, a roll-up is refused until recovery runs."""
    store = InMemoryRollUpStore()
    service = RollUpService(store)
    service.record(_attribution(decision_id="1", shortfall="1.20"), _key())

    store.simulate_torn_write()
    restarted = RollUpService(store)

    try:
        restarted.rollup(Dimension.STRATEGY, "S-1")
    except StoreNotVerified as refusal:
        assert "recover" in str(refusal).lower()
    else:
        raise AssertionError("an unverified store served a roll-up")


def test_recovery_discards_the_partial_record_and_then_serves():
    """Recovery reconstructs a verified state and serving resumes."""
    store = InMemoryRollUpStore()
    service = RollUpService(store)
    service.record(_attribution(decision_id="1", shortfall="1.20"), _key())
    store.simulate_torn_write()

    restarted = RollUpService(store)
    restarted.recover()
    rollup = restarted.rollup(Dimension.STRATEGY, "S-1")

    assert rollup.population_computed == 1
    assert rollup.shortfall_total == Decimal("1.20")


def test_recovery_is_verified_not_assumed():
    """A clean store is servable without a recovery call, but a torn one is not."""
    clean = RollUpService(InMemoryRollUpStore())
    clean.record(_attribution(decision_id="1", shortfall="1.20"), _key())
    assert clean.rollup(Dimension.STRATEGY, "S-1").population_computed == 1
