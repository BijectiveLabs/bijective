# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""Integration gates for the C4 PostgreSQL adapter; skipped when the store is not configured.
Covers exact decimal persistence, cross-adapter roll-up equivalence and atomicity.
"""

import os
from decimal import Decimal

import pytest

pg8000_dbapi = pytest.importorskip("pg8000.dbapi")

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
)
from bijective.attribution.store_postgres import PostgresRollUpStore

_PORT = os.environ.get("FF_PGPORT")
_PASS = os.environ.get("FF_PGPASS")

pytestmark = pytest.mark.skipif(
    not (_PORT and _PASS),
    reason="relational store not configured (FF_PGPORT / FF_PGPASS unset)",
)


def _connect():
    """Open a connection from environment details; production resolves credentials via the secrets broker."""
    return pg8000_dbapi.connect(
        host="127.0.0.1",
        port=int(_PORT),
        user="ff",
        password=_PASS,
        database="ff_attribution",
    )


@pytest.fixture()
def store():
    """A clean store per test."""
    conn = _connect()
    adapter = PostgresRollUpStore(conn)
    cursor = conn.cursor()
    cursor.execute("TRUNCATE ff_attribution, ff_refusal")
    conn.commit()
    cursor.close()
    try:
        yield adapter
    finally:
        adapter.close()


def _attribution(decision_id: str, shortfall: str, *, fee_available: bool = True) -> Attribution:
    fee = (
        Measured(value=Decimal("0.20"), availability=Availability.OBSERVED,
                 resolution=Resolution.NANOSECOND)
        if fee_available
        else Measured(value=None, availability=Availability.UNAVAILABLE,
                      reason="liquidity not observed")
    )
    return Attribution(
        decision_id=decision_id,
        instrument="BTC-PERP",
        filled_qty=Decimal("10"),
        benchmark_px=Decimal("100.00"),
        realised_px=Decimal("100.10"),
        shortfall=Decimal(shortfall),
        components=CostComponents(
            crossing=Measured(value=Decimal("1.00"), availability=Availability.OBSERVED,
                              resolution=Resolution.NANOSECOND),
            fee=fee,
            ack_residual=Measured(value=Decimal("0.00"), availability=Availability.OBSERVED,
                                 resolution=Resolution.NANOSECOND),
            unexplained=Decimal("0.00"),
        ),
        inputs_resolution=Resolution.NANOSECOND,
        computed_from=(f"F-{decision_id}",),
    )


def _key(strategy="S-1", instrument="BTC-PERP", venue="V-1", period="2026-07") -> RollUpKey:
    return RollUpKey(strategy_id=strategy, instrument=instrument, venue=venue, period=period)


def test_a_money_figure_survives_the_round_trip_exactly(store):
    """An awkward decimal is stored and read back with no loss."""
    awkward = "1234.56789012345678"
    store.put_attribution(_key(), _attribution("1", awkward))

    (recovered_key, recovered), = store.committed()

    assert recovered.shortfall == Decimal(awkward)
    assert recovered.shortfall.as_tuple() == Decimal(awkward).as_tuple()
    assert recovered_key == _key()


def test_rollup_over_the_store_equals_rollup_over_the_reference(store):
    """The same records yield the same aggregate whichever adapter holds them."""
    records = [
        (_attribution("1", "1.20"), _key()),
        (_attribution("2", "1.20"), _key()),
        (_attribution("3", "1.00", fee_available=False), _key()),
    ]

    durable = RollUpService(store)
    reference = RollUpService(InMemoryRollUpStore())
    for attribution, key in records:
        durable.record(attribution, key)
        reference.record(attribution, key)
    durable.record_refusal(_key(), "no decision-time book")
    reference.record_refusal(_key(), "no decision-time book")

    a = durable.rollup(Dimension.STRATEGY, "S-1")
    b = reference.rollup(Dimension.STRATEGY, "S-1")

    assert a.shortfall_total == b.shortfall_total
    assert a.population_total == b.population_total
    assert a.population_computed == b.population_computed
    assert a.coverage == b.coverage
    assert a.components.fee.availability == b.components.fee.availability
    assert a.components.fee.availability is Availability.UNAVAILABLE
    assert a.components.crossing.value == b.components.crossing.value


def test_an_aborted_write_leaves_nothing_behind(store):
    """Atomicity is the recovery guarantee: a torn write is never persisted."""
    store.put_attribution(_key(), _attribution("committed", "1.20"))

    conn = _connect()
    try:
        cursor = conn.cursor()
        cursor.execute(
            "INSERT INTO ff_attribution (strategy_id, instrument, venue, period, payload) "
            "VALUES (%s, %s, %s, %s, %s)",
            ("S-1", "BTC-PERP", "V-1", "2026-07", "{}"),
        )
        conn.rollback()
    finally:
        conn.close()

    remaining = store.committed()
    assert len(remaining) == 1
    assert remaining[0][1].decision_id == "committed"
    assert store.torn() is False
