# 0015: A backtest runs the order path on simulated venues

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-22; item 6 extended to paper the same day. Stage 1 built; stage 2 is a later decision |
| Extends | The run modes (`docs/concepts/architecture.md`), the sandbox (decision 0003), the simulated venue |

## Context

A backtest ran the strategy over a recording and stopped at its proposals. Fills, positions and
P&L appeared only in a paper run, which needs live data, three processes and a configuration. A
first run therefore answered "nothing happened". The control engine accepted `--mode backtest`
but opened no venue, so every proposal there had no eligible venue.

Nautilus (`BacktestEngine`) and LEAN run one process with a simulated clock and a simulated
exchange, and deliver fills to the strategy as events. Two rules of this platform constrain the
same design here: the strategy engine never links the venue or the risk code (gate `Agent environment`), and the control engine is the only issuer of a clearance (gate `Order path closed`). The backtest therefore stays two programs, as paper is.

## Decision

1. **The venues are simulated, as in paper.** A backtest opens one simulated venue per configured
   venue, from its profile, its declared sandbox balances and its declared fees, under the paper
   model. No venue protocol is opened and no credential is read. One configuration serves paper
   and backtest.
2. **A record of its own.** A backtest writes `control-backtest`, `limits-backtest` and reads
   `proposals-backtest` and `commands-backtest` in the directory `--store` names. The recorded
   market is read from `--market-store`, never written. `--market` names the stream whose
   positions the proposals were decided at.
3. **The recording drives time.** The clock moves to each market entry's receipt time (the venue's
   time where none was recorded) and never back. A pass runs at a position where a proposal was
   decided, once five seconds of recorded time have passed since the last pass (the live
   heartbeat), and at the window's end, followed by one closing pass that records the last fills.
4. **A pass reads up to its step.** The driving stream is read through the step's position, every
   other market and facts stream through its instant, and the proposals decided at or before the
   position. Nothing later reaches the pass.
5. **Orders settle in the pass that placed them**, in paper and backtest alike: with no latency
   modelled, an order meets the book it was decided on and every later update, as the simulated
   venue's step states.
6. **Started halted, resumed by a recorded command.** A backtest or paper node starts halted like
   every node. With no operator registry nobody could sign a resume, so at start, when no command
   is waiting, it appends one resume (actor `unattended`) to its own command stream, and the
   first pass carries it out as any command. Neither places capital. Given a registry, it appends
   nothing and checks signatures as any node does. A live node refuses to start without one.
7. **A proposal without a position is refused at start.** A whole-exchange run states no decided
   position, so it cannot be stepped; the start names the count and asks for a stepwise run.
8. **The result.** At its end the control engine prints, per venue: orders at the venue, fills,
   positions, the proposals refused and why the first was, cash at the start and the end, the
   change in equity and the largest drawdown from a peak, both measured per currency at each
   pass's marks.
9. **One command.** `bijective-backtest <strategy> --config <node configuration>` (or `"config"`
   in `bijective.json`) runs the strategy, copies the proposals the run wrote into
   `<store>/run/backtests/<first>-<last>`, and runs the control engine over them. A set of
   strategies is not placed.

## Stage 2 (later decision)

Lockstep: the supervisor waits for the control engine to process position `p` before sending the
next event, so order feedback reaches the strategy at the same position as in paper.

## Consequences

- Stage 1 tells the strategy nothing: the research run has ended before the order path runs. The
  result says so. A strategy whose decisions depend on its fills is measured correctly only in
  paper until stage 2.
- Two backtests of one record and one recording write byte-identical control streams
  (`Replay parity` gate).
- A backtest no longer opens no venue: it opens simulated ones only. The `Replay parity` gate
  checks that it reads no credential and reaches no venue.
- Paper orders now settle in the pass that placed them rather than the next one.
- Equity is per currency; a book in two currencies is not converted in the result.
- The first pass reads the recorded streams whole, as a node's first pass does.
