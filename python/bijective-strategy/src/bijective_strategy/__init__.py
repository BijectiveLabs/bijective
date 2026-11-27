# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""The protocol a strategy speaks: one JSON object per line in both directions.

A convenience over stdin and stdout; the supervisor decides what a proposal may say.
"""

from __future__ import annotations

import json
import sys
from dataclasses import dataclass
from typing import Iterator, TextIO

__all__ = [
    "Book",
    "EndOfWindow",
    "Fact",
    "Resumed",
    "Unrecognised",
    "answered",
    "events",
    "propose",
    "read_event",
]


@dataclass(frozen=True)
class Book:
    """A book update with its position in the market's history.

    Fields stay as sent: scaled integers are decimal strings, never floats.
    """

    position: str
    instrument: str
    venue_sequence: str | None
    venue_ts_ms: str
    bid_price: str
    bid_quantity: str
    ask_price: str
    ask_quantity: str
    bid_depth: int
    ask_depth: int
    #: The venue the top was quoted on. None where the stream names no venue.
    venue: str | None = None
    #: UTC nanoseconds on this node's clock when the top arrived: one clock for every venue.
    #: None on a top recorded before receive times were kept.
    received_at_ns: str | None = None


@dataclass(frozen=True)
class InstrumentRules:
    """One instrument's rules on the session's venue, at the instrument's own scales, as strings.

    A limit price must be a multiple of `price_tick` and a quantity a multiple of `quantity_lot`,
    or the order is refused before it leaves.
    """

    instrument: str
    price_scale: int
    quantity_scale: int
    price_tick: str
    quantity_lot: str
    minimum_size: str
    minimum_notional: str
    trading: bool
    #: What the instrument is (spot, perpetual, ...). None where the venue declared none.
    kind: str | None = None
    quote_currency: str | None = None
    settlement_currency: str | None = None
    #: The asset a quantity is counted in. None where the venue's declaration predates it.
    base_currency: str | None = None


@dataclass(frozen=True)
class Venue:
    """The venue a session reads and what it charges, sent once before the first top.

    Fees are basis points at the platform's 8-decimal scale, as strings. None means no fee
    declaration stands for the venue, which is not a fee of zero.
    """

    venue: str
    taker_fee_bps_scaled: str | None = None
    maker_fee_bps_scaled: str | None = None
    fee_source: str | None = None
    instruments: tuple[InstrumentRules, ...] = ()


@dataclass(frozen=True)
class OrderUpdate:
    """What became of one of this strategy's proposals, in a live session.

    `proposal_position` is where the proposal landed in the proposal stream. `status` is one of
    refused, submitted, accepted, rejected, unknown, filled, cancel_requested, canceled, expired or
    reconciling. `unknown` means the order may or may not be at the venue: neither placed nor failed.
    A fill carries its quantity and price at the venue's scales, as strings.
    """

    proposal_position: str
    status: str
    #: UTC nanoseconds on the platform clock. From submitted to the venue's answer is its latency.
    recorded_at_ns: str = ""
    client_id: str | None = None
    reason: str = ""
    fill_id: str | None = None
    fill_quantity_scaled: str | None = None
    fill_price_scaled: str | None = None


@dataclass(frozen=True)
class Fact:
    """A fact about an instrument besides its top of book: a trade, liquidation, funding rate,
    open interest, mark or index price or long/short ratio, named by `kind`.

    `fact_position` is a position on the venue's facts stream, not one a decision names. `values`
    holds the kind's own fields as sent: scaled figures as strings, each `_scale` as a number.
    """

    kind: str
    fact_position: str
    instrument: str
    venue: str | None
    exchange_ts_ns: str | None
    received_at_ns: str
    values: dict[str, object]


@dataclass(frozen=True)
class EndOfWindow:
    """A bounded run's window is exhausted. A live session is never sent this."""


@dataclass(frozen=True)
class Resumed:
    """Where a live session's events pick up, and what ran while nothing was listening."""

    from_position: str
    missed: object
    #: The venue whose market this resume point is on. A session of several markets is sent one
    #: per market. None where the stream names no venue.
    venue: str | None = None
    #: The stream the position is on (`market-<venue>` or `facts-<venue>`).
    stream: str | None = None


@dataclass(frozen=True)
class Unrecognised:
    """An event this version does not know. Returned, not raised, so an older strategy keeps running.

    """

    raw: str


def read_event(
    line: str,
) -> Book | Venue | OrderUpdate | Fact | EndOfWindow | Resumed | Unrecognised:
    """One line the supervisor sent, read."""
    try:
        message = json.loads(line)
    except ValueError:
        return Unrecognised(line)
    if not isinstance(message, dict):
        return Unrecognised(line)

    match message.get("event"):
        case "book":
            try:
                return Book(
                    position=message["position"],
                    instrument=message["instrument"],
                    venue_sequence=message.get("venue_sequence"),
                    venue_ts_ms=message["venue_ts_ms"],
                    bid_price=message["bid_price"],
                    bid_quantity=message["bid_quantity"],
                    ask_price=message["ask_price"],
                    ask_quantity=message["ask_quantity"],
                    bid_depth=message["bid_depth"],
                    ask_depth=message["ask_depth"],
                    venue=message.get("venue"),
                    received_at_ns=message.get("received_at_ns"),
                )
            except KeyError:
                # A partial book is not half a book. Deciding on one would be a guess.
                return Unrecognised(line)
        case "venue" if isinstance(message.get("venue"), str):
            try:
                return Venue(
                    venue=message["venue"],
                    taker_fee_bps_scaled=message.get("taker_fee_bps_scaled"),
                    maker_fee_bps_scaled=message.get("maker_fee_bps_scaled"),
                    fee_source=message.get("fee_source"),
                    instruments=tuple(_rules(r) for r in message.get("instruments", [])),
                )
            except (KeyError, TypeError, ValueError):
                # Partial rules are not rules. A price placed on a guessed increment is refused anyway.
                return Unrecognised(line)
        case "order" if isinstance(message.get("proposal_position"), str):
            return OrderUpdate(
                proposal_position=message["proposal_position"],
                status=message.get("status", ""),
                recorded_at_ns=message.get("recorded_at_ns", ""),
                client_id=message.get("client_id"),
                reason=message.get("reason", ""),
                fill_id=message.get("fill_id"),
                fill_quantity_scaled=message.get("fill_quantity_scaled"),
                fill_price_scaled=message.get("fill_price_scaled"),
            )
        case "end_of_window":
            return EndOfWindow()
        case "resumed":
            return Resumed(
                from_position=message.get("from_position", ""),
                missed=message.get("missed"),
                venue=message.get("venue"),
                stream=message.get("stream"),
            )
        case "fact" if isinstance(message.get("kind"), str):
            return _fact(message)
        case _:
            return Unrecognised(line)


_FACT_COMMON = ("event", "kind", "fact_position", "instrument", "venue", "exchange_ts_ns",
                "received_at_ns")


def _fact(message: dict[str, object]) -> Fact:
    """Every kind reads the same way, so a kind added later reaches a strategy without a release."""
    return Fact(
        kind=str(message["kind"]),
        fact_position=str(message.get("fact_position", "")),
        instrument=str(message.get("instrument", "")),
        venue=_text(message.get("venue")),
        exchange_ts_ns=_text(message.get("exchange_ts_ns")),
        received_at_ns=str(message.get("received_at_ns", "")),
        values={k: v for k, v in message.items() if k not in _FACT_COMMON},
    )


def _rules(rules: dict[str, object]) -> InstrumentRules:
    """Named fields only: a field a later platform adds is ignored, not a reason to fail."""
    return InstrumentRules(
        instrument=str(rules["instrument"]),
        price_scale=int(str(rules["price_scale"])),
        quantity_scale=int(str(rules["quantity_scale"])),
        price_tick=str(rules["price_tick"]),
        quantity_lot=str(rules["quantity_lot"]),
        minimum_size=str(rules["minimum_size"]),
        minimum_notional=str(rules["minimum_notional"]),
        trading=bool(rules["trading"]),
        kind=_text(rules.get("kind")),
        quote_currency=_text(rules.get("quote_currency")),
        settlement_currency=_text(rules.get("settlement_currency")),
        base_currency=_text(rules.get("base_currency")),
    )


def _text(value: object) -> str | None:
    return None if value is None else str(value)


def events(
    stream: TextIO | None = None,
) -> Iterator[Book | Venue | OrderUpdate | Fact | EndOfWindow | Resumed | Unrecognised]:
    """Every event, as it arrives."""
    for line in stream if stream is not None else sys.stdin:
        line = line.strip()
        if line:
            yield read_event(line)


def propose(
    *,
    desk: str,
    instrument: str,
    side: str,
    quantity_scaled: str,
    price_scaled: str | None = None,
    cap_scaled: str | None = None,
    client_id: str,
    decided_at_ns: str,
    decided_at_position: str | None = None,
    venue: str | None = None,
    time_in_force: str | None = None,
    post_only: bool = False,
    stop_price_scaled: str | None = None,
    expire_at_ns: str | None = None,
    reduce_only: bool = False,
    parent: str | None = None,
    oco: str | None = None,
    out: TextIO | None = None,
) -> None:
    """Say what to do. Keyword-only, because seven positional strings is how the wrong two swap.

    `venue` names the venue the decision was made for (an arbitrage leg, a venue's fee or
    liquidity); the order is admitted there or refused, never moved to another. Left out, the
    desk's venue preference decides. `time_in_force` is `gtc` (left out), `ioc`, `fok`, `day`,
    `at_the_open` or `at_the_close`; `post_only` asks for an order that only rests. A venue that
    cannot honour them refuses the order rather than send it under other terms.
    `stop_price_scaled` makes it a stop: held until the price reaches it (a buy when it rises to
    it, a sell when it falls), then a limit order at `price_scaled` or, without one, a market
    order bounded by `cap_scaled`. `reduce_only` asks for an order that only reduces what is held:
    an exit or a protective stop, which a halted platform leaves working. `expire_at_ns` (UTC
    nanoseconds) has the node cancel the order if it still works then; it needs a time in force
    that rests.
    `parent` names the `client_id` of an earlier proposal: this one is held until that one's
    orders fill, then placed for the quantity filled, and must be `reduce_only`. `oco` names a
    group under the same parent: once one member's order fills, the others are cancelled. A
    bracket is an entry and two reduce-only children, a stop and a take-profit, in one group.
    """
    message: dict[str, object] = {
        "emit": "proposal",
        "desk": desk,
        "instrument": instrument,
        "side": side,
        "quantity_scaled": quantity_scaled,
        "client_id": client_id,
        "decided_at_ns": decided_at_ns,
    }
    # Absent stays absent: null is not the same as a field that was not sent.
    if price_scaled is not None:
        message["price_scaled"] = price_scaled
    if cap_scaled is not None:
        message["cap_scaled"] = cap_scaled
    if decided_at_position is not None:
        message["decided_at_position"] = decided_at_position
    if venue is not None:
        message["venue"] = venue
    if time_in_force is not None:
        message["time_in_force"] = time_in_force
    if post_only:
        message["post_only"] = True
    if stop_price_scaled is not None:
        message["stop_price_scaled"] = stop_price_scaled
    if expire_at_ns is not None:
        message["expire_at_ns"] = expire_at_ns
    if reduce_only:
        message["reduce_only"] = True
    if parent is not None:
        message["parent"] = parent
    if oco is not None:
        message["oco"] = oco
    _write(message, out)


def answered(out: TextIO | None = None) -> None:
    """This event is finished with. The stepwise exchange waits for it."""
    _write({"emit": "answered"}, out)


def _write(message: dict[str, object], out: TextIO | None) -> None:
    stream = out if out is not None else sys.stdout
    stream.write(json.dumps(message, separators=(",", ":")))
    stream.write("\n")
    # Flushed on every line. A buffered answer leaves the supervisor waiting until the wall bound.
    stream.flush()
