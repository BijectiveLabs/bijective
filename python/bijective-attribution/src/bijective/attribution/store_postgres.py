# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""C4 store adapter over PostgreSQL (pg8000). Stores base records only; roll-up arithmetic stays in store.py.
Decimals as strings, bound parameters only, one atomic transaction per write. T2 is superseded.
"""

from __future__ import annotations

import json
from decimal import Decimal

import pg8000.dbapi

from bijective.attribution.calculator import (
    Attribution,
    Availability,
    CostComponents,
    Measured,
    Resolution,
)
from bijective.attribution.store import RefusalMarker, RollUpKey

# Static DDL: names are literals, never interpolated. Only row values are bound.
_DDL = (
    """
    CREATE TABLE IF NOT EXISTS ff_attribution (
        id          BIGSERIAL PRIMARY KEY,
        strategy_id TEXT NOT NULL,
        instrument  TEXT NOT NULL,
        venue       TEXT NOT NULL,
        period      TEXT NOT NULL,
        payload     TEXT NOT NULL
    )
    """,
    """
    CREATE TABLE IF NOT EXISTS ff_refusal (
        id          BIGSERIAL PRIMARY KEY,
        strategy_id TEXT NOT NULL,
        instrument  TEXT NOT NULL,
        venue       TEXT NOT NULL,
        period      TEXT NOT NULL,
        reason      TEXT NOT NULL
    )
    """,
)


def _measured_to_json(measured: Measured) -> dict:
    """Serialise a measurement, keeping its decimal value exact as a string."""
    return {
        "value": None if measured.value is None else str(measured.value),
        "availability": measured.availability.name,
        "resolution": measured.resolution.name if measured.resolution else None,
        "basis": measured.basis,
        "reason": measured.reason,
    }


def _measured_from_json(raw: dict) -> Measured:
    """Reconstruct a measurement, restoring the exact decimal from its string form."""
    return Measured(
        value=None if raw["value"] is None else Decimal(raw["value"]),
        availability=Availability[raw["availability"]],
        resolution=Resolution[raw["resolution"]] if raw["resolution"] else None,
        basis=raw["basis"],
        reason=raw["reason"],
    )


def _attribution_to_payload(attribution: Attribution) -> str:
    """Serialise a full attribution to a JSON string with every money value as text."""
    document = {
        "decision_id": attribution.decision_id,
        "instrument": attribution.instrument,
        "filled_qty": str(attribution.filled_qty),
        "benchmark_px": str(attribution.benchmark_px),
        "realised_px": str(attribution.realised_px),
        "shortfall": str(attribution.shortfall),
        "components": {
            "crossing": _measured_to_json(attribution.components.crossing),
            "fee": _measured_to_json(attribution.components.fee),
            "ack_residual": _measured_to_json(attribution.components.ack_residual),
            "unexplained": str(attribution.components.unexplained),
        },
        "inputs_resolution": attribution.inputs_resolution.name,
        "computed_from": list(attribution.computed_from),
    }
    return json.dumps(document)


def _attribution_from_payload(payload: str) -> Attribution:
    """Reconstruct a full attribution from its stored JSON, decimals exact."""
    document = json.loads(payload)
    components = document["components"]
    return Attribution(
        decision_id=document["decision_id"],
        instrument=document["instrument"],
        filled_qty=Decimal(document["filled_qty"]),
        benchmark_px=Decimal(document["benchmark_px"]),
        realised_px=Decimal(document["realised_px"]),
        shortfall=Decimal(document["shortfall"]),
        components=CostComponents(
            crossing=_measured_from_json(components["crossing"]),
            fee=_measured_from_json(components["fee"]),
            ack_residual=_measured_from_json(components["ack_residual"]),
            unexplained=Decimal(components["unexplained"]),
        ),
        inputs_resolution=Resolution[document["inputs_resolution"]],
        computed_from=tuple(document["computed_from"]),
    )


class PostgresRollUpStore:
    """Durable `RollUpStore`: one connection, one atomic transaction per write. Use as a context manager."""

    def __init__(self, connection: "pg8000.dbapi.Connection") -> None:
        # Injected so credential handling stays with the caller and never lives in this module.
        self._conn = connection
        self._ensure_schema()

    def _ensure_schema(self) -> None:
        """Create the two append-only tables if they are absent. Idempotent DDL."""
        cursor = self._conn.cursor()
        try:
            for statement in _DDL:
                cursor.execute(statement)
            self._conn.commit()
        finally:
            cursor.close()

    def put_attribution(self, key: RollUpKey, attribution: Attribution) -> None:
        """Insert one attribution as a single atomic transaction, decimals stored exactly."""
        cursor = self._conn.cursor()
        try:
            cursor.execute(
                "INSERT INTO ff_attribution "
                "(strategy_id, instrument, venue, period, payload) "
                "VALUES (%s, %s, %s, %s, %s)",
                (
                    key.strategy_id,
                    key.instrument,
                    key.venue,
                    key.period,
                    _attribution_to_payload(attribution),
                ),
            )
            self._conn.commit()
        finally:
            cursor.close()

    def put_refusal(self, marker: RefusalMarker) -> None:
        """Insert one refusal marker as a single atomic transaction."""
        cursor = self._conn.cursor()
        try:
            cursor.execute(
                "INSERT INTO ff_refusal "
                "(strategy_id, instrument, venue, period, reason) "
                "VALUES (%s, %s, %s, %s, %s)",
                (
                    marker.key.strategy_id,
                    marker.key.instrument,
                    marker.key.venue,
                    marker.key.period,
                    marker.reason,
                ),
            )
            self._conn.commit()
        finally:
            cursor.close()

    def committed(self) -> list[tuple[RollUpKey, Attribution]]:
        """Read every stored attribution back in insertion order, decimals reconstructed exactly."""
        cursor = self._conn.cursor()
        try:
            cursor.execute(
                "SELECT strategy_id, instrument, venue, period, payload "
                "FROM ff_attribution ORDER BY id"
            )
            rows = cursor.fetchall()
        finally:
            cursor.close()
        return [
            (
                RollUpKey(strategy_id=r[0], instrument=r[1], venue=r[2], period=r[3]),
                _attribution_from_payload(r[4]),
            )
            for r in rows
        ]

    def refusals(self) -> list[RefusalMarker]:
        """Read every stored refusal marker back in insertion order."""
        cursor = self._conn.cursor()
        try:
            cursor.execute(
                "SELECT strategy_id, instrument, venue, period, reason "
                "FROM ff_refusal ORDER BY id"
            )
            rows = cursor.fetchall()
        finally:
            cursor.close()
        return [
            RefusalMarker(
                key=RollUpKey(strategy_id=r[0], instrument=r[1], venue=r[2], period=r[3]),
                reason=r[4],
            )
            for r in rows
        ]

    def torn(self) -> bool:
        """Always false: the database rolls back an incomplete write, so a torn base record is never visible."""
        return False

    def discard_torn(self) -> None:
        """No-op: there is nothing to discard, for the reason stated in `torn`. Satisfies the port."""
        return None

    def close(self) -> None:
        """Release the underlying connection."""
        self._conn.close()

    def __enter__(self) -> "PostgresRollUpStore":
        return self

    def __exit__(self, *exc_info: object) -> None:
        self.close()
