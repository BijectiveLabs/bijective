# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""C3 - the attribution calculator: execution cost against the decision-time benchmark, by cause.
Pure and clock-free; declare or refuse, never coerce; no binary floats, so results are reproducible.
"""

from __future__ import annotations

import decimal
from dataclasses import dataclass, field
from decimal import Decimal
from enum import Enum

# Fixed local decimal context, so the host process's context cannot change a reported figure.
_CONTEXT = decimal.Context(prec=34, rounding=decimal.ROUND_HALF_EVEN)


class Resolution(Enum):
    """Fidelity of a timestamp, kept rather than normalised: a second-grade record must not pass as nanosecond."""

    NANOSECOND = 0
    MICROSECOND = 1
    MILLISECOND = 2
    SECOND = 3

    @property
    def rank(self) -> int:
        """Ordering position from finest (0) to coarsest, used to pick the weakest input."""
        return int(self.value)


class Availability(Enum):
    """Whether a value was seen, inferred, or is simply not obtainable."""

    OBSERVED = "observed"
    DERIVED = "derived"
    UNAVAILABLE = "unavailable"


class SettlementState(Enum):
    """Whether the venue can still change this execution. A filled order may be voided or reopened by a correction."""

    #: The venue may still correct or reverse this execution.
    PROVISIONAL = "provisional"
    #: The venue can no longer change it. Figures derived from it are final.
    SETTLED = "settled"
    #: The venue has reversed it. It did not happen, and nothing may be derived from it.
    VOIDED = "voided"


class LiquiditySide(Enum):
    """Which side provided liquidity. Needed to attribute a fee: maker and taker are charged differently."""

    MAKER = "maker"
    TAKER = "taker"
    UNKNOWN = "unknown"


@dataclass(frozen=True)
class Instant:
    """A point in nanoseconds carrying the fidelity of its source; `resolution` says how much of it is real."""

    value: int
    resolution: Resolution
    source: str


@dataclass(frozen=True)
class Measured:
    """A quantity with an accurate statement of what is known about it; construction rejects misleading combinations."""

    value: Decimal | None
    availability: Availability
    resolution: Resolution | None = None
    basis: str | None = None
    reason: str | None = None

    def __post_init__(self) -> None:
        if self.availability is Availability.UNAVAILABLE:
            if self.value is not None:
                raise ValueError("an unavailable measurement must carry no value")
            if not self.reason:
                raise ValueError("an unavailable measurement must state its reason")
        if self.availability is Availability.DERIVED and not self.basis:
            raise ValueError("a derived measurement must state its basis")
        if self.availability is Availability.OBSERVED and self.value is None:
            raise ValueError("an observed measurement must carry a value")


@dataclass
class BookSnapshot:
    """Top of book at one instant. Mutable: captured upstream by C1 and never mutated here."""

    instrument: str
    best_bid: Decimal
    best_bid_qty: Decimal
    best_ask: Decimal
    best_ask_qty: Decimal
    mid: Decimal
    observed_at: Instant


@dataclass
class DecisionRecord:
    """What the strategy decided, when, and the book then. `book` is None without a signal: refuse, never substitute."""

    decision_id: str
    correlation_id: str
    strategy_id: str
    instrument: str
    decided_at: Instant
    book: BookSnapshot | None


@dataclass
class Fill:
    """One execution, in the canonical shape the substrate adapters populate."""

    fill_id: str
    correlation_id: str
    instrument: str
    side: str  # "BUY" or "SELL"
    last_qty: Decimal
    last_px: Decimal
    fee: Decimal
    liquidity: LiquiditySide
    ts_event: Instant
    #: Fee currency. A fee outside the position's cost currency is missing from substrate PnL.
    fee_currency: str = ""
    #: Defaults to provisional: treating an unsettled fill as final is the error.
    settlement: SettlementState = SettlementState.PROVISIONAL
    #: Venue-echoed id, distinct from the platform's correlation_id. A fill may carry only this.
    client_order_id: str = ""


@dataclass
class CapabilityDeclaration:
    """What the substrate adapter states it can supply. Read from the declaration, never inferred from populated fields."""

    liquidity: Availability
    acknowledgement: Availability
    fee: Availability


@dataclass
class VenueReference:
    """Venue economics for reconstructing a fee. Unused in Wave 1; in the signature so later use keeps call sites."""

    venue: str
    fee_schedule: dict = field(default_factory=dict)
    #: Instrument cost currency (quote, base or settlement by contract type). Never assumed.
    cost_currency: str = ""


@dataclass
class CostComponents:
    """The named causes, plus the part that is not accounted for."""

    crossing: Measured
    fee: Measured
    ack_residual: Measured
    unexplained: Decimal


@dataclass
class Attribution:
    """The complete result for one decision and the executions that followed it."""

    decision_id: str
    instrument: str
    filled_qty: Decimal
    benchmark_px: Decimal
    realised_px: Decimal
    shortfall: Decimal
    components: CostComponents
    inputs_resolution: Resolution
    computed_from: tuple[str, ...]
    #: A group is only as final as its least final member.
    settlement: SettlementState = SettlementState.PROVISIONAL
    #: Currency the attributed fee is denominated in.
    fee_currency: str = ""
    #: Whether substrate realized PnL counts this fee. When false the operator paid a cost PnL never shows.
    fee_counted_in_substrate_pnl: bool = False

    @property
    def is_final(self) -> bool:
        """Whether this figure can be relied on as settled. A property, so it cannot drift from the settlement state."""
        return self.settlement is SettlementState.SETTLED


class InsufficientEvidence(Exception):
    """Raised when no accurate figure can be produced; no partial numeric result may escape."""

    def __init__(self, reason: str, missing: list[str] | None = None) -> None:
        super().__init__(reason)
        self.reason = reason
        self.missing = missing or []


def _coarsest(resolutions: list[Resolution]) -> Resolution:
    """Least precise resolution among the inputs: a derived figure is no better than its weakest input."""
    return max(resolutions, key=lambda r: r.rank)


def attribute(
    decision: DecisionRecord,
    fills: list[Fill],
    reference: VenueReference,
    declaration: CapabilityDeclaration,
) -> Attribution:
    """Attribute the cost of `fills` against the market state at `decision`.
    Raises `InsufficientEvidence` when no figure can be produced from the evidence.
    """
    if not fills:
        raise InsufficientEvidence(
            "no executions to attribute", ["fills"]
        )

    book = decision.book
    if book is None:
        # No captured book means no benchmark. A later price would answer a different question.
        raise InsufficientEvidence(
            "no decision-time book, so no arrival benchmark exists",
            ["benchmark", "decision.book"],
        )

    if book.observed_at.value > decision.decided_at.value:
        # The benchmark may only use information that existed at the decision (no look-ahead).
        raise InsufficientEvidence(
            "look-ahead: the book was observed after the decision instant",
            ["benchmark"],
        )

    for fill in fills:
        if fill.instrument != decision.instrument:
            raise InsufficientEvidence(
                "fill instrument does not match the decision instrument",
                ["instrument"],
            )

    # A voided fill did not happen. Attributing it would invent a cost never paid.
    if any(f.settlement is SettlementState.VOIDED for f in fills):
        raise InsufficientEvidence(
            "a fill was voided by the venue, so no cost may be attributed to it",
            ["settlement"],
        )

    # Fees in different currencies cannot be summed without a rate this component must not invent.
    fee_currencies = {(f.fee_currency or "").strip() for f in fills}
    if len(fee_currencies) > 1:
        raise InsufficientEvidence(
            "fills carry fees in differing currencies, which cannot be summed without a "
            "conversion rate this component does not hold",
            ["fee_currency"],
        )

    sides = {f.side.upper() for f in fills}
    if not sides.issubset({"BUY", "SELL"}):
        raise InsufficientEvidence(
            "unrecognised order side, so the direction of cost cannot be determined",
            ["side"],
        )
    if len(sides) > 1:
        raise InsufficientEvidence(
            "fills for one decision carry opposing sides, which cannot share a "
            "single benchmark",
            ["side"],
        )

    with decimal.localcontext(_CONTEXT):
        filled_qty = sum((f.last_qty for f in fills), Decimal(0))
        if filled_qty <= 0:
            raise InsufficientEvidence(
                "filled quantity is not positive, so no cost can be attributed",
                ["filled_qty"],
            )

        # Volume-weighted: partial fills are the norm on a live venue.
        notional = sum((f.last_px * f.last_qty for f in fills), Decimal(0))
        realised_px = notional / filled_qty

        side = fills[0].side.upper()
        benchmark_px = book.mid

        # Crossing cost is mid to touch: what the strategy paid for demanding immediacy.
        if side == "BUY":
            touch_px = book.best_ask
            crossing_per_unit = touch_px - benchmark_px
            residual_per_unit = realised_px - touch_px
            price_cost = (realised_px - benchmark_px) * filled_qty
        else:
            touch_px = book.best_bid
            crossing_per_unit = benchmark_px - touch_px
            residual_per_unit = touch_px - realised_px
            price_cost = (benchmark_px - realised_px) * filled_qty

        fee_total = sum((f.fee for f in fills), Decimal(0))
        shortfall = price_cost + fee_total

        resolutions = [decision.decided_at.resolution, book.observed_at.resolution]
        resolutions.extend(f.ts_event.resolution for f in fills)
        inputs_resolution = _coarsest(resolutions)

        crossing = Measured(
            value=crossing_per_unit * filled_qty,
            availability=Availability.OBSERVED,
            resolution=inputs_resolution,
        )

        # Gate (C5) holds the rule in one place. Imported here because it depends on this module's records.
        from bijective.attribution.capability import gate

        # A fee is attributable only when the liquidity side was observed: maker and taker differ in cost.
        fee_capability = gate("fee", declaration)
        # The declaration says what the adapter can supply, the record what it did. Both must hold.
        if any(f.liquidity is LiquiditySide.UNKNOWN for f in fills):
            fee_capability = Availability.UNAVAILABLE

        if fee_capability is Availability.UNAVAILABLE:
            fee = Measured(
                value=None,
                availability=Availability.UNAVAILABLE,
                reason=(
                    "liquidity side is not observed on this substrate, so the fee "
                    "cannot be attributed to maker or taker"
                ),
            )
        elif fee_capability is Availability.DERIVED:
            fee = Measured(
                value=fee_total,
                availability=Availability.DERIVED,
                resolution=inputs_resolution,
                basis="fee or liquidity side is reconstructed, not observed",
            )
        else:
            fee = Measured(
                value=fee_total,
                availability=Availability.OBSERVED,
                resolution=inputs_resolution,
            )

        # Interval effect: second-grade records cannot measure it, so it is unavailable, not approximate.
        ack_capability = gate("ack_residual", declaration)
        if ack_capability is Availability.UNAVAILABLE:
            ack_residual = Measured(
                value=None,
                availability=Availability.UNAVAILABLE,
                reason="acknowledgement state is not observed on this substrate",
            )
        elif inputs_resolution.rank >= Resolution.SECOND.rank:
            ack_residual = Measured(
                value=None,
                availability=Availability.UNAVAILABLE,
                reason=(
                    "input resolution is second-grade, which cannot support an "
                    "interval measurement"
                ),
            )
        elif ack_capability is Availability.DERIVED:
            ack_residual = Measured(
                value=residual_per_unit * filled_qty,
                availability=Availability.DERIVED,
                resolution=inputs_resolution,
                basis="acknowledgement state is reconstructed, not observed",
            )
        else:
            ack_residual = Measured(
                value=residual_per_unit * filled_qty,
                availability=Availability.OBSERVED,
                resolution=inputs_resolution,
            )

        # Reported in full: it tells an operator how much of the cost the model explains.
        explained = Decimal(0)
        for component in (crossing, fee, ack_residual):
            if component.value is not None:
                explained += component.value
        unexplained = shortfall - explained

        # One execution the venue can still correct makes the whole figure provisional.
        settlement = (
            SettlementState.SETTLED
            if all(f.settlement is SettlementState.SETTLED for f in fills)
            else SettlementState.PROVISIONAL
        )

        # A fee outside the cost currency never reaches substrate PnL. Attributed and flagged so it cannot vanish.
        fee_currency = next(iter(fee_currencies))
        fee_counted = bool(
            fee_currency
            and reference.cost_currency
            and fee_currency == reference.cost_currency
        )

        return Attribution(
            decision_id=decision.decision_id,
            instrument=decision.instrument,
            filled_qty=filled_qty,
            benchmark_px=benchmark_px,
            realised_px=realised_px,
            shortfall=shortfall,
            components=CostComponents(
                crossing=crossing,
                fee=fee,
                ack_residual=ack_residual,
                unexplained=unexplained,
            ),
            inputs_resolution=inputs_resolution,
            computed_from=tuple(f.fill_id for f in fills),
            settlement=settlement,
            fee_currency=fee_currency,
            fee_counted_in_substrate_pnl=fee_counted,
        )
