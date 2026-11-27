# 0013: Linked orders held by the platform

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-21 |
| Extends | Stops and reduce-only proposals (decision 0012), the proposal and the order path |

## Context

A strategy could place an entry and a protective stop, but not tie them: the stop could only be
proposed once the strategy had seen the entry fill, and nothing cancelled a take-profit when its
stop filled. Venues offer linked orders natively (Alpaca bracket, OCO and OTO; Bybit take-profit
and stop-loss attached to an order), but their legs carry identifiers the venue assigns, which
reconciliation reads as orders placed outside the platform, and each venue names the link its
own way. Execution engines that support many venues, NautilusTrader's order manager among them,
hold contingencies themselves: every leg is an ordinary order with its own client identifier, a
child is submitted through risk when its parent fills, and a fill on one leg of a
one-cancels-other group cancels the others.

## Decision

1. **The platform holds the link; every leg is an ordinary order the node names.** No adapter
   changes, and reconciliation reads no order it did not place.
2. **A proposal states its link.** `parent` names the client identifier of an earlier proposal:
   the proposal is held until that proposal's orders fill. `oco` names a group, scoped to the
   parent: once an order of one member fills, the other members' orders are cancelled and never
   released. A bracket is an entry and two children, a stop and a take-profit, in one group. A
   proposal with a parent must be reduce-only, and the parent must have been placed or held. A
   proposal that reuses a linked identifier, or joins a group a member of which has filled, is
   refused.
3. **Held, then released through the same gates.** A linked proposal is admitted by its desk and
   recorded whole as `OrderHeld` (control layout 12, proposal layout 7), so a restart still holds
   it. Each pass releases what is due: for a child, what its parent has filled less what it has
   had released, never more than it asked; for a member without a parent, all of it at once.
   Each release passes the collar, the reduce-only check and risk like a new proposal, under a
   new client identifier. Releases are counted from the intents on the record, so a restart
   between a fill and a release releases once. Nothing is released while the trading state does
   not admit the order, while the limits are unrecovered or while a flatten is in force: the
   proposal stays held. A refused release is not tried again in the same process.
4. **One-cancels-other.** The orders of a member displaced by a peer's fill are withdrawn by the
   sweep, whatever the trading state. A reduce-only check does not count a peer's orders against
   a member's, because they are alternatives.
5. **When the node stops.** On a venue that enforces reduce-only, every leg rests there, and the
   venue keeps two legs from turning the position over. On one that does not, only legs with a
   stop rest at the venue; a group's limit leg without a stop is held until a fresh mark reaches
   its price, then released. A stopped node leaves the stop working and nothing that could fill
   twice.
6. **A cancel-orders or flatten command** ends every held proposal not yet released in full.
7. **Not built now:** one-updates-the-other (each venue amends differently), trailing stops,
   iceberg and scheduled execution. Expiry at a time follows through the sweep.

## Consequences

- A strategy can place a bracket in one turn; the stop reaches the venue on the pass that reads
  the entry's fill.
- A parent that fills in parts releases its children in parts: one order per leg per part, each
  counted against the venue's limit on resting orders (Bybit: 10 conditional orders per symbol).
- A held proposal is told to its strategy when it is released or refused; it is not told while it
  is held.
- A halt keeps a child held: the protection is placed when the node is resumed or reducing.
- A take-profit on a venue that does not enforce reduce-only is placed only when the mark reaches
  it, so it fills later than a resting order would.
- Records written before this read with no parent and no group.
