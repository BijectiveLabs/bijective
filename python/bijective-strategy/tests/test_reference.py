# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

"""The reference strategies decide what their rules say, and decide it the same way every time."""

from __future__ import annotations

import importlib.util
import io
import subprocess
import sys
from decimal import Decimal
from pathlib import Path
from types import ModuleType

from bijective_strategy import Book

REFERENCE = Path(__file__).resolve().parent.parent / "examples" / "reference"
MINUTE_MS = 60_000


def load(name: str) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, REFERENCE / f"{name}.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # Registered first: a dataclass resolves its annotations through its module.
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def book(minute: int, mid: int, position: int = 0) -> Book:
    return Book(
        position=str(position),
        instrument="btc-perp",
        venue_sequence=None,
        venue_ts_ms=str(1_788_000_000_000 + minute * MINUTE_MS),
        bid_price=str(mid - 1),
        bid_quantity="5",
        ask_price=str(mid + 1),
        ask_quantity="5",
        bid_depth=1,
        ask_depth=1,
    )


def minutes(mids: list[int]) -> list[Book]:
    """One top per minute, plus one more so the last minute closes."""
    tops = [book(m, mid, m) for m, mid in enumerate(mids)]
    return tops + [book(len(mids), mids[-1], len(mids))]


def decided(core: object, tops: list[Book]) -> list[tuple[str, int, int]]:
    out = []
    for top in tops:
        order = core.on_book(top)  # type: ignore[attr-defined]
        if order is not None:
            out.append((order.side, order.quantity_scaled, order.limit_price_scaled))
    return out


def test_zscore_sells_a_spike_at_the_ask_and_buys_back_when_it_reverts() -> None:
    module = load("zscore_reversion")
    core = module.ZScoreReversion(unit_quantity_scaled=10, lookback=5)
    noise = [1000, 1002, 1000, 1002, 1000]
    orders = decided(core, minutes(noise + [1050, 1001]))
    # Decided on the first top of the next minute, at that top: the spike closed, the price moved on.
    assert orders == [("sell", 10, 1002), ("buy", 10, 1000)]


def test_zscore_refuses_thresholds_that_cannot_be_ordered() -> None:
    module = load("zscore_reversion")
    try:
        module.ZScoreReversion(unit_quantity_scaled=1, entry_z=Decimal(1), exit_z=Decimal(2))
    except ValueError:
        return
    raise AssertionError("an exit wider than the entry was accepted")


def test_momentum_buys_a_rise_and_sells_after_its_hold() -> None:
    module = load("momentum")
    core = module.Momentum(unit_quantity_scaled=10, horizon=2, window=6, hold=2)
    noise = [1000, 1001, 1000, 1001, 1000, 1001, 1000]
    orders = decided(core, minutes(noise + [1020, 1040, 1041, 1042]))
    assert orders[0] == ("buy", 10, 1039)
    assert orders[1][0] == "sell"
    assert len(orders) == 2


def run_main(name: str, lines: str) -> str:
    library = Path(__file__).resolve().parent.parent / "src"
    done = subprocess.run(
        [sys.executable, str(REFERENCE / f"{name}.py")],
        input=lines,
        capture_output=True,
        text=True,
        env={"PYTHONPATH": str(library)},
        check=True,
    )
    return done.stdout


def test_each_reference_strategy_answers_every_event_and_is_deterministic() -> None:
    stream = io.StringIO()
    for top in minutes([1000, 1002, 1000, 1002, 1000, 1050, 1001] * 3):
        stream.write(
            '{"event":"book","position":"%s","instrument":"btc-perp","venue_ts_ms":"%s",'
            '"bid_price":"%s","bid_quantity":"5","ask_price":"%s","ask_quantity":"5",'
            '"bid_depth":1,"ask_depth":1}\n'
            % (top.position, top.venue_ts_ms, top.bid_price, top.ask_price)
        )
    stream.write('{"event":"end_of_window"}\n')
    for name in ("zscore_reversion", "momentum"):
        first = run_main(name, stream.getvalue())
        assert first == run_main(name, stream.getvalue()), name
        assert first.count('"emit":"answered"') == 22, name
