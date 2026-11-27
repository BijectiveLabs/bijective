# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""Reference strategy: mean reversion on the minute mid, for exercising the platform end to end.

A reference strategy proves the order path, the risk gate and the record under load. It is not a
recommendation: the research this platform descends from falsified z-score reversion on its own
data, and the platform's promotion stages exist to say so again on yours.

The decision core (`ZScoreReversion`) is pure: it is given book tops and returns at most one order
each time, and it reads no clock, no file and no network. `main` connects it to the strategy
protocol. Every figure is a scaled integer or a `Decimal`, never a float.

Rule: the close of each minute (the last mid seen in it) is compared with the previous `lookback`
closes. Above `entry_z` standard deviations the strategy wants to be short, below `-entry_z` long,
and within `exit_z` of the mean flat again. It rests a limit order at the touch for the difference
between the position it wants and the position it has asked for. It is not told of fills, so the
position it holds is the one it asked for.
"""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass
from decimal import Decimal

from bijective_strategy import Book, EndOfWindow, answered, events, propose

MINUTE_MS = 60_000


@dataclass(frozen=True)
class Order:
    side: str
    quantity_scaled: int
    limit_price_scaled: int
    reason: str


class ZScoreReversion:
    """Decides on book tops. Precondition: tops arrive in stream order for one instrument."""

    def __init__(
        self,
        *,
        unit_quantity_scaled: int,
        lookback: int = 20,
        entry_z: Decimal = Decimal(2),
        exit_z: Decimal = Decimal("0.5"),
        bucket_ms: int = MINUTE_MS,
    ) -> None:
        if lookback < 3 or unit_quantity_scaled <= 0 or bucket_ms <= 0:
            raise ValueError("lookback >= 3, a positive unit and a positive bucket are required")
        if not Decimal(0) <= exit_z < entry_z:
            raise ValueError("thresholds must satisfy 0 <= exit_z < entry_z")
        self.unit = unit_quantity_scaled
        self.entry_z = entry_z
        self.exit_z = exit_z
        self.bucket_ms = bucket_ms
        self._closes: deque[int] = deque(maxlen=lookback)
        self._bucket: int | None = None
        self._last_mid: int | None = None
        self._held = 0

    def on_book(self, book: Book) -> Order | None:
        """At most one order, decided when a minute closes. A side that is empty decides nothing."""
        bid, ask = int(book.bid_price), int(book.ask_price)
        if bid <= 0 or ask <= 0:
            return None
        bucket = int(book.venue_ts_ms) // self.bucket_ms
        closed = self._last_mid if self._bucket is not None and bucket != self._bucket else None
        self._bucket = bucket
        self._last_mid = (bid + ask) // 2
        if closed is None:
            return None
        wanted = self._wanted(closed)
        self._closes.append(closed)
        if wanted is None or wanted == self._held:
            return None
        return self._order(wanted, bid, ask)

    def _wanted(self, close: int) -> int | None:
        if len(self._closes) < (self._closes.maxlen or 0):
            return None
        window = [Decimal(c) for c in self._closes]
        mean = sum(window, Decimal(0)) / len(window)
        variance = sum(((c - mean) ** 2 for c in window), Decimal(0)) / (len(window) - 1)
        if variance <= 0:
            return None
        z = (Decimal(close) - mean) / variance.sqrt()
        if z >= self.entry_z:
            return -1
        if z <= -self.entry_z:
            return 1
        if abs(z) <= self.exit_z:
            return 0
        return self._held

    def _order(self, wanted: int, bid: int, ask: int) -> Order:
        change = wanted - self._held
        self._held = wanted
        side = "buy" if change > 0 else "sell"
        # Resting at the near touch: a buy at the bid, a sell at the ask.
        price = bid if change > 0 else ask
        return Order(side, abs(change) * self.unit, price, f"target {wanted:+d}")


def main() -> None:
    core = ZScoreReversion(unit_quantity_scaled=1_000)
    sent = 0
    for event in events():
        if isinstance(event, Book):
            order = core.on_book(event)
            if order is not None:
                sent += 1
                propose(
                    desk="reference",
                    instrument=event.instrument,
                    side=order.side,
                    quantity_scaled=str(order.quantity_scaled),
                    price_scaled=str(order.limit_price_scaled),
                    client_id=f"zscore-{sent}",
                    decided_at_ns=str(int(event.venue_ts_ms) * 1_000_000),
                    decided_at_position=event.position,
                )
            answered()
        elif isinstance(event, EndOfWindow):
            break


if __name__ == "__main__":
    main()
