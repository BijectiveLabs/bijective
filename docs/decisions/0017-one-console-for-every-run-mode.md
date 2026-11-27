# 0017: One console for every run mode

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-24. Stage 1 built; stage 2 built for strategy performance, 2026-11-24 |
| Extends | The run modes (`docs/concepts/architecture.md`), the console (`docs/concepts/observability.md`), the backtest (decision 0015) |

## Context

A client trades a strategy live while it researches the same strategy, or a variant, in paper and
in backtests over the same recorded market. The question was whether research needs panels of its
own beside the trading ones.

It does not. Every mode writes the same record: a paper node writes `control-paper` and
`limits-paper`, a backtest `control-backtest` and `limits-backtest`, in the formats the live node
writes (`bijective_serialization::signal::node`), from the same strategy program and the same order
path. A fold over one is a fold over the others. Separate research panels would be a second fold of
the same events that drifts from the first, which is the backtest drift the single code path exists
to prevent.

Two defects stood in the way. The console named its streams by three flags that nothing tied
together, so a console could read a paper record while its control page signed commands for the
live node: a halt pressed over a paper record halted the node that places capital. And nothing on
the page said which mode's record was shown.

## Decision

1. **One set of panels, one table format, for every mode.** Research and trading are the same
   views of different runs. The run is a dimension, never a separate surface.
2. **A console serves one run, named by its mode.** `--mode live|paper|backtest` (live unless
   given) selects that mode's streams together. The three stream flags are removed: a console
   cannot read one node's record and command another.
3. **Commands go to the node whose record is shown.** The control page signs for the selected
   mode's command stream, as `bijective issue --paper` does.
4. **The mode is on every page.** It leads the strip (`mode`), and so every table reader and
   `/metrics` sees it. The Bijective Pro console app marks a record that is not live on every view.
5. **Detections are kept per mode** like the node's own streams: `detections` for live,
   `detections-paper` and `detections-backtest` for the others. A detector names the mode it reads.
6. **One market source.** The market a live node records is the one a backtest replays
   (`--market-store`, decision 0015); nothing is copied into a research format.

## Stage 2 (later decision)

Several runs of one strategy side by side (live against paper, a backtest against the live record)
in one view, joined by the owner every intent carries (desk and strategy). It is built when a view
needs it, as several runs served by one console; until then one console per run gives the same
tables.

Built for strategy performance (2026-11-24), the view that needs it first: a strategy measured in
backtest, in paper and live is only evidence when the three are read together, and the gap between
them is a figure of its own. The console reads the performance stream of every mode in its store,
and of every backtest record under `run/backtests/<window>` (where `bijective-backtest` writes
each), and names each row's run: `live`, `paper`, `backtest <window>`. Only performance is read
across runs; every other table, and every command, stays with the console's own mode (rule 3).

## Consequences

- A paper or backtest record is read with the same console, the same panels and the same app views
  as live, and is marked as such everywhere it is shown.
- `--control`, `--limits` and `--commands` are no longer accepted by the console; `--mode` replaces
  them. No released version carried them.
- A backtest's record directory is read by pointing `--store` at it with `--mode backtest`.
