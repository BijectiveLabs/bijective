# 0010: Price collar and the flatten command

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-19 |
| Extends | Desk admission, the operator command set and the order path's clearance |

## Context

Admission checked a limit price against the venue's tick, the lot, the minimum notional and the
desk's per-order ceiling, never against the market. A strategy that scaled a price wrongly could
send a buy far above the ask, and a marketable limit sweeps the book up to the ceiling.

The operator could halt, reduce and cancel orders, but not close a position. In an emergency the
positions were closed by hand at the venue, outside the record. And a reduce-only order still
needed a clearance from a clear evaluation, so under a breach, an unjudged limit or stale figures,
which is when positions are closed, no reduction could be placed at all.

## Decision

1. **A desk collars limit prices.** A limit price may cross the instrument's mark by at most the
   desk's `price_collar_bps` (default 500, at least 1, at most 10000): a buy above the mark or a
   sell below it by more is refused (`OutsidePriceCollar`) before the order leaves. A price that
   does not cross rests without harm and is not bounded. The mark is the one the node values with
   (the mid of the market stream); a limit price with no mark, or with a mark older than the
   node's freshness bound, is refused (`NoMarkForCollar`, `MarkTooOldForCollar`). A market order
   carries no price the venue sees; its cap and the per-order ceiling bound it.
2. **`flatten` closes what the record holds.** A signed operator command, open to an operator and
   a risk officer, not an observer. Carried out, it moves the node to reducing (a halted node
   stays halted) and, after the pass's sweep, places one reduce-only market order per open
   position on the other side, for the quantity the reduce-only orders already working do not
   close. Each order is written as an intent before the submit and settled after, like every
   order; its decision identifier is `flatten:<command id>`, which names no proposal.
3. **A flatten order is cleared by the command, not by the figures.** A breach, an unjudged limit
   or stale figures are why positions are closed, so they cannot be what stops the closing. The
   order path judges each flatten order on its record (reduce-only, within what is held), on the
   venue's instrument rules and on its order rate; nothing is drawn from a budget. The clearance
   it carries (`Clearance::for_operator_reduction`) is minted in one place, which the `Order path closed` gate holds.
4. **The record is the quantity, and an untrusted record is not used.** A venue whose latest
   evidence read failed or whose reconciliation diverged is reported, not flattened: closing by a
   quantity the record cannot vouch for could open the other side where the venue does not
   enforce reduce-only. The operator closes it at the venue.
5. **A flatten stays in force until a resume** (amended by decision 0012, which first made it
   act once): it orders every open order cancelled, and each pass closes what the record still
   holds that working orders do not, so a remainder an immediate order left is closed on a later
   pass. A flatten order refused once is not tried again in the same process.
6. **A command that is refused is carried out by nobody.** The node acts on venue withdrawal,
   venue admission and flatten only when the command was authenticated, permitted and new. Before
   this record, a refused venue admission was still applied by the node.
7. **A flatten keeps its own closing orders** (amended 2026-11-23). The sweep of a non-active node
   withdraws reduce-only orders on a venue that does not enforce reduce-only, and it withdrew the
   flatten's own market order while that order waited for its fill (16 seconds on a paper
   equity venue); the flatten then counted it closed and placed it again, so each pass raced a
   cancel against the fill. An order placed under the decision identifier of the flatten in force
   is now left working on a venue whose record is trusted, enforced or not. Withdrawing it removed
   no risk: it was sized from the record against the other working reduce-only orders, and its
   cancel could still lose the race to its fill. It is withdrawn as before once the venue's record
   is untrusted, or when a later flatten or a cancel-orders command orders it cancelled.

## Consequences

- A fat-finger limit price is refused at admission with both prices and the collar in the
  refusal; a desk trading thin markets widens its collar in its declaration.
- An operator can close every position through the platform, recorded and signed, in any state.
- Strategies may not use identifiers starting `flatten:`; a proposal that does is refused.
- Not covered: a position the record does not hold (it halts the node as external first), and
  venue-side protection that survives the node, which order types resting at the venue provide.
