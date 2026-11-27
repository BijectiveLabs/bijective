# 0018: A paper venue is rebuilt from its record

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-24. Built |
| Extends | Paper mode (`docs/concepts/simulated_venue.md`), the sandbox (decision 0003), one console for every run mode (decision 0017) |

## Context

A paper node's simulated venue lived only in the process. A restart opened a new one with the
declared balances and no orders, under a record that still held the old run. A drill on
2026-11-24 showed what followed: the cash was back at its starting figure, an order the record
held as working was unknown to the venue, reconciliation still read aligned because the venue
said nothing about that order, and the halted node's sweep asked the venue to cancel it on every
pass, refused each time as no open order.

A client researching a strategy in paper beside its live trading (decision 0017) runs paper for
days. A paper run that cannot survive a restart measures nothing past the first one.

## Decision

1. **At start, each simulated venue takes back what its record says it held.** The node replays
   its paper record first, as every node does; the simulated venue is then given that venue's
   ledger and takes back every order the record saw accepted, with the venue's own identifiers,
   every fill in the record, and the cash and positions the fills made.
2. **Working orders work again.** An order the record holds as working rests again (a stop with no
   fill is held until its price is reached, since the record does not say whether it triggered)
   and a working buy reserves what its remainder may cost. An order whose cancel the record asked
   for is cancelled on the next settle.
3. **What the record does not know is not invented.** An order never seen accepted, or whose fate
   is unknown, is not taken back: reconciliation reads it as absent, as at a venue that never held
   it. The book is not restored; it is rebuilt from the market the node reads next.
4. **New identifiers follow the restored ones**, so an order or a fill placed after a restart is
   never confused with one before it.
5. **Paper only.** A backtest writes a record directory of its own and replays its window from the
   start (decision 0015); live venues hold their own state.

## Consequences

- A paper run continues across restarts with the same positions and cash, and its evidence
  reconciles with the record from the first pass.
- A run that stops on `--drill-seconds` or a stop signal (SIGTERM, an interrupt) stops between
  passes and reconciles once more, so every fill the simulated venue made is in the record
  (2026-11-24). A process killed outright still loses a fill made after its last pass: that order
  rests again and meets the market anew.
- The start prints what each venue took back: orders, fills and working orders.
