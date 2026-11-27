# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""Reference strategy: long-only time-series momentum on the minute mid, for exercising the platform.

A reference strategy proves the order path, the risk gate and the record under load. It is not a
recommendation. Crypto's published time-series momentum is long-biased (Liu and Tsyvinski, Review
of Financial Studies, 2021); replay it on your own data before believing it.

The decision core (`Momentum`) is pure: book tops in, at most one order out, no clock, file or
network. Every figure is a scaled integer or a `Decimal`, never a float.

Rule: at each minute close, the log return over the last `horizon` minutes is compared with the
standard deviation of one-minute log returns over `window` minutes, scaled to the horizon. At or
above `entry_z` it goes long one unit at the bid; after `hold` minutes it sells the unit at the ask.
It is not told of fills, so the position it holds is the one it asked for.
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


class Momentum:
    """Decides on book tops. Precondition: tops arrive in stream order for one instrument."""

    def __init__(
        self,
        *,
        unit_quantity_scaled: int,
        horizon: int = 15,
        window: int = 60,
        entry_z: Decimal = Decimal(1),
        hold: int = 15,
        bucket_ms: int = MINUTE_MS,
    ) -> None:
        if not 1 <= horizon < window or hold < 1 or unit_quantity_scaled <= 0 or bucket_ms <= 0:
            raise ValueError("1 <= horizon < window, a positive hold, unit and bucket are required")
        self.unit = unit_quantity_scaled
        self.horizon = horizon
        self.entry_z = entry_z
        self.hold = hold
        self.bucket_ms = bucket_ms
        self._closes: deque[int] = deque(maxlen=window + 1)
        self._bucket: int | None = None
        self._last_mid: int | None = None
        self._held_for: int | None = None

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
        self._closes.append(closed)
        if self._held_for is not None:
            self._held_for += 1
            if self._held_for >= self.hold:
                self._held_for = None
                return Order("sell", self.unit, ask, f"held {self.hold} minutes")
            return None
        z = self._momentum_z()
        if z is None or z < self.entry_z:
            return None
        self._held_for = 0
        return Order("buy", self.unit, bid, f"momentum z {z:.3f}")

    def _momentum_z(self) -> Decimal | None:
        closes = [Decimal(c) for c in self._closes]
        if len(closes) < (self._closes.maxlen or 0):
            return None
        steps = [(b / a).ln() for a, b in zip(closes, closes[1:])]
        mean = sum(steps, Decimal(0)) / len(steps)
        variance = sum(((s - mean) ** 2 for s in steps), Decimal(0)) / (len(steps) - 1)
        if variance <= 0:
            return None
        moved = (closes[-1] / closes[-1 - self.horizon]).ln()
        return moved / (variance.sqrt() * Decimal(self.horizon).sqrt())


def main() -> None:
    core = Momentum(unit_quantity_scaled=1_000)
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
                    client_id=f"momentum-{sent}",
                    decided_at_ns=str(int(event.venue_ts_ms) * 1_000_000),
                    decided_at_position=event.position,
                )
            answered()
        elif isinstance(event, EndOfWindow):
            break


if __name__ == "__main__":
    main()
