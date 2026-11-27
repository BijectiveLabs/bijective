# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""C4 - the attribution store and roll-up over a storage port, with coverage stated per aggregate.
A total is reported only when every member carried the component; an unverified store serves nothing.
"""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal
from enum import Enum
from typing import Protocol

from bijective.attribution.calculator import (
    Attribution,
    Availability,
    CostComponents,
    Measured,
)


class Dimension(Enum):
    """The roll-up axes, each a field of `RollUpKey`; an enum so an unknown axis fails at the call site."""

    STRATEGY = "strategy_id"
    INSTRUMENT = "instrument"
    VENUE = "venue"
    PERIOD = "period"


@dataclass(frozen=True)
class RollUpKey:
    """The coordinates an attribution is filed under, kept outside it so the calculator stays pure."""

    strategy_id: str
    instrument: str
    venue: str
    period: str


@dataclass(frozen=True)
class RefusalMarker:
    """Records that a decision in scope produced no attribution, so coverage can count refusals."""

    key: RollUpKey
    reason: str


@dataclass
class RollUp:
    """An aggregate over one dimension value, with its coverage stated."""

    dimension: Dimension
    key: str
    population_total: int
    population_computed: int
    shortfall_total: Decimal
    components: CostComponents
    coverage: Decimal


class StoreNotVerified(Exception):
    """Raised when a roll-up is requested from a store left unverified by an interrupted write."""


class RollUpStore(Protocol):
    """The persistence contract C4 depends on; kept small so the store stays replaceable."""

    def put_attribution(self, key: RollUpKey, attribution: Attribution) -> None:
        """Persist one attribution under its key, atomically - it commits whole or not at all."""
        ...

    def put_refusal(self, marker: RefusalMarker) -> None:
        """Persist one refusal marker, so a refused decision is counted in coverage."""
        ...

    def committed(self) -> list[tuple[RollUpKey, Attribution]]:
        """Return every whole, committed attribution with its key, in insertion order."""
        ...

    def refusals(self) -> list[RefusalMarker]:
        """Return every refusal marker recorded."""
        ...

    def torn(self) -> bool:
        """Report whether an interrupted write left the store in an unverified state."""
        ...

    def discard_torn(self) -> None:
        """Clear the torn state after discarding any partial write (records are regenerable)."""
        ...


class InMemoryRollUpStore:
    """Reference adapter: in-memory, no dependency; models an interrupted write for recovery tests."""

    def __init__(self) -> None:
        self._committed: list[tuple[RollUpKey, Attribution]] = []
        self._refusals: list[RefusalMarker] = []
        self._torn = False

    def put_attribution(self, key: RollUpKey, attribution: Attribution) -> None:
        """Append a whole record (atomic in memory; begin-write then commit in a real store)."""
        self._committed.append((key, attribution))

    def put_refusal(self, marker: RefusalMarker) -> None:
        """Append a refusal marker to the recorded set."""
        self._refusals.append(marker)

    def committed(self) -> list[tuple[RollUpKey, Attribution]]:
        """Return a copy of the committed records, so a caller cannot mutate the store."""
        return list(self._committed)

    def refusals(self) -> list[RefusalMarker]:
        """Return a copy of the recorded refusals."""
        return list(self._refusals)

    def torn(self) -> bool:
        """Report the torn flag set by an interrupted write (see `simulate_torn_write`)."""
        return self._torn

    def discard_torn(self) -> None:
        """Clear the torn flag. The partial write is discarded, not repaired: records are regenerable upstream."""
        self._torn = False

    # Test interface, not part of the `RollUpStore` port.
    def simulate_torn_write(self) -> None:
        """Model a process interrupted between beginning and committing a write; it persisted no record."""
        self._torn = True


def _aggregate_component(members: list[Measured], name: str) -> Measured:
    """Combine one component across a group: unavailable if any member lacks it, derived if any was derived."""
    if not members:
        # A zero would read as "this cost was zero" rather than "nothing could be measured".
        return Measured(
            value=None,
            availability=Availability.UNAVAILABLE,
            reason=f"no computed records in group, so {name} cannot be reported",
        )
    missing = sum(1 for m in members if m.availability is Availability.UNAVAILABLE)
    if missing:
        return Measured(
            value=None,
            availability=Availability.UNAVAILABLE,
            reason=(
                f"{missing} of {len(members)} members lack an attributed {name}, "
                f"so the group total would understate the cost"
            ),
        )
    total = sum((m.value for m in members), Decimal(0))
    if any(m.availability is Availability.DERIVED for m in members):
        return Measured(
            value=total,
            availability=Availability.DERIVED,
            basis=f"at least one member's {name} was reconstructed, not observed",
        )
    return Measured(value=total, availability=Availability.OBSERVED)


class RollUpService:
    """Records attributions and refusals, and serves aggregates over a verified store."""

    def __init__(self, store: RollUpStore) -> None:
        self._store = store
        # Derived from the store's state, never optimistically true: a torn store is unverified until recovery.
        self._verified = not store.torn()

    def record(self, attribution: Attribution, key: RollUpKey) -> None:
        """File a computed attribution under the coordinates it rolls up by."""
        self._store.put_attribution(key, attribution)

    def record_refusal(self, key: RollUpKey, reason: str) -> None:
        """Record that a decision in scope produced no attribution, so coverage counts it."""
        self._store.put_refusal(RefusalMarker(key=key, reason=reason))

    def recover(self) -> None:
        """Reconstruct a verified state after an interrupted write by discarding the partial write."""
        if self._store.torn():
            self._store.discard_torn()
        self._verified = True

    def rollup(self, dimension: Dimension, value: str) -> RollUp:
        """Aggregate every record whose `dimension` equals `value`; refuses on an unverified store.
        Coverage counts refusals in the denominator, so it states the share of the population represented.
        """
        if not self._verified:
            raise StoreNotVerified(
                "the store was interrupted and has not been recovered; "
                "call recover() before serving a roll-up"
            )

        field_name = dimension.value
        members = [
            attribution
            for key, attribution in self._store.committed()
            if getattr(key, field_name) == value
        ]
        refused = sum(
            1
            for marker in self._store.refusals()
            if getattr(marker.key, field_name) == value
        )

        computed = len(members)
        total = computed + refused

        shortfall_total = sum((m.shortfall for m in members), Decimal(0))
        components = CostComponents(
            crossing=_aggregate_component([m.components.crossing for m in members], "crossing"),
            fee=_aggregate_component([m.components.fee for m in members], "fee"),
            ack_residual=_aggregate_component(
                [m.components.ack_residual for m in members], "acknowledgement residual"
            ),
            unexplained=sum((m.components.unexplained for m in members), Decimal(0)),
        )

        # Exact: an integer ratio as a Decimal, never a float.
        coverage = Decimal(computed) / Decimal(total) if total else Decimal(0)

        return RollUp(
            dimension=dimension,
            key=value,
            population_total=total,
            population_computed=computed,
            shortfall_total=shortfall_total,
            components=components,
            coverage=coverage,
        )
