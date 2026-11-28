# Risk and Limits

Risk evaluation is a mandatory gate between a strategy's proposal and the venue. It is the one part
of the platform whose failure can cost capital directly, so it is pure, deterministic computation
over inputs that pass freshness and completeness checks.

| Crate | Role |
|---|---|
| `bijective-limits` | Limit vocabulary, evaluation arithmetic, safe-state selection, the pre-trade check. Zero dependencies |
| `bijective-risk::limit_state` | Durable limit state: declarations, consumption and releases as events, rebuilt by replay |
| `bijective-risk::valuation` | The exposure figures limits are judged against |
| `bijective-risk::desks` | The desk register, capital allocation and order admission |
| `bijective-authority` | Which role may take which operator action |

## Arithmetic

- **No floating point.** Money, price and exposure are scaled integers. `Amount::SCALE` is 8.
- **Overflow is refused, never wrapped.** A wrapped exposure is a breach that reads as compliant;
  a wrapped subtraction turns a small release into an enormous budget.
- **Notional rounds away from zero** when narrowing to the platform scale: a reservation short of
  the true cost would permit more than the limit declares.

## Declaring a limit

`Limit::declare` is the only constructor, and it refuses two kinds of limit that would appear on the
register while protecting nothing:

- **Unit mismatch.** The threshold is not in the unit this kind of limit measures. The check runs at
  run time because limits arrive from configuration.
- **Unreachable.** No observation could exceed the threshold.

The set of limit kinds is fixed, so a limit cannot be declared whose figure the platform does not
know how to source. A limit is a maximum: an observation exactly at the limit is compliant.
`net_exposure` is judged by its magnitude, long or short: a net short of 10 breaches a maximum of
5, as a net long of 10 does. The figure itself keeps its sign. Every other kind is judged signed,
so a day's gain is not a loss.

## Scope

Limits apply at six levels:

```
portfolio -> desk -> venue -> strategy -> underlying -> instrument
```

Portfolio, venue, underlying and instrument figures sum what the venues hold. Desk and strategy figures come
from the platform's placement record (see Desk and strategy exposure below).

A deployment with one desk and one venue uses the same model with levels collapsed.

## Judging

A comparison across units refuses rather than returning "no breach". A limit that cannot be
evaluated is treated as failed, never as passed.

## Safe states

The trading state is `Active`, `Reducing` or `Halted`. The state chosen for a breach follows from
what the breach means, never from what a state is called. A breach escalates to `Halted` when
either holds:

1. **Scope.** A breach at portfolio, desk or strategy level spans many instruments. `Reducing`
   would leave every flat instrument open to new exposure, so it does not answer the breach. Only a
   breach scoped to an instrument already held is served by `Reducing`.
2. **Venue semantics.** Where a venue clamps reduce-only at fill time instead of refusing it at
   placement, `Reducing` guarantees more than the venue keeps.

`Reducing` means "do not extend existing exposure": it admits only a reduce-only order that
reduces the held position on its venue, and that order still needs a clearance from a passing
evaluation, so a reduction during a standing breach is refused (fail closed). Every other order is
refused. A venue-level breach currently halts.

Reconciliation drives `Halted` too: a pass that diverges, or a venue whose evidence cannot be read,
while the platform trades halts it with `ReconciliationDiverged` or `EvidenceUnread` as the reason.
A live node re-reads every venue's positions every five minutes, and the venues with anything
unsettled every minute.

Driving a safe state is verified twice:

- `drive_safe_state` issues the request and reads the state back. A call that returned is evidence
  that a call returned and nothing more.
- `verify_exposure_did_not_increase` checks that the state had the effect its name guarantees. Where
  the later snapshot carries no figure to compare, the result is reported as unverifiable, never as
  satisfied.

Escalation never relaxes: a request for a lighter state than the one in force is ignored. Returning
to `Active` is a separate, authorised operator action that no automatic path can take.

## The pre-trade check

The pre-trade check judges, at the same instant, only what it already holds:

- **Fail closed.** Every uncertainty resolves to a halt.
- **Bounded.** No I/O, no lock, no wall clock, no allocation on the decision path; allocation
  happens once at construction. Work is linear in the number of declared limits. The
  `Pre-trade check bound` gate refuses the crate any dependency.
- **Ordered by uncertainty.** It checks, in order: never primed; silent (nothing received within
  the declared bound, measured on the receiving side); a limit it cannot judge; a breach. It returns
  an action, not a report.

## Durable limit state

There is no separate limit database. Declarations, consumption and releases are events in the
event store, and consumed budget is a replay of them.

- A restart does not restore spent budget.
- Before replay completes, the budget is `Unrecovered`, which is a different answer from
  `Exhausted`. Both stop a spend.
- A consumption is written to the store before the in-memory state moves. A consumption that cannot
  be recorded spends nothing.
- A corrupt history stops the replay. A release larger than the consumption it releases is refused.
- A release names the decision whose reservation it returns (`LimitReleased` version 2), and the
  replay holds, per decision and scope, what was taken and what is still held. A decision cannot
  return more than it took. A version-1 release names no decision.

## Valuation

Limits are judged against the platform's own valuation:

- One position the platform cannot value empties the whole snapshot. A gross exposure computed from
  the positions it could value would understate the book, and an absent figure is judged as
  unevaluable, which halts.
- A valuation is as old as its oldest ingredient. A venue that cannot be asked makes the snapshot
  stale.
- Positions are valued at mid. Gross and net exposure are kept as separate figures.
- Unrealised profit is the mark against each position's entry price, signed by side. It exists only
  when the whole book was valued, for the same reason the snapshot does.
- Every figure is in one currency ([decision 0005](../decisions/0005-reporting-currency.md)). A
  node may declare a reporting currency and, for each other currency it holds, a spot market whose
  mark is the rate:

  ```json
  "valuation": {
    "currency": "usd",
    "rates": { "usdt": { "instrument": "<id>", "max_age_ns": "300000000000" } },
    "markets": [{ "name": "<stream name>", "profile": "<profile path>" }]
  }
  ```

  `markets` names market streams read only for rates (a data engine run with `--stream-name`),
  with the profile that catalogues their instruments. A rate market that is not catalogued, not
  spot, or does not price that currency in the reporting currency stops the node at start. A rate
  with no positive mark, or older than its `max_age_ns`, leaves the position unvalued. Money
  limits are read in the reporting currency. Without `valuation`, the first position valued fixes the book's
  currency and a position settling in another is unvalued.

## Daily loss

`daily_loss` is declared at portfolio scope only: a loss by venue or instrument has no figure and
would halt the node on every pass. The loss is the opposite of the day's result, and the result is
what the day's fills realised plus how far the unrealised profit has moved since the day opened.
A gain is a negative loss, which no maximum is below.

- The day is the UTC day. Its baseline, the book's unrealised profit at the first complete
  valuation of the day, is written to the control stream as `DayOpened`, so a restart does not
  measure from a later baseline. The first record of a day stands.
- A node first started in the middle of a day measures from that moment, not from the day's open.
- Without a baseline, an unvalued book, or a fill on an instrument that cannot be priced, no
  figure is produced. The limit is then unjudged, and an unjudged limit halts the node.
- Every fill's fee is part of the figure when it is paid in an asset the book can be valued in:
  its currency, or one with a declared rate, compared without regard to letter case (a venue
  writes `USDT`, a profile `usdt`). A fee left unstated, with an empty asset, in an asset with no
  rate, or a rebate (a negative figure), is left out and counted, and the control engine logs how
  many: a paid fee left out makes the figure smaller than the loss by that fee, which is why the
  count is reported. Rebates are left out until a venue's rebate sign is confirmed; crediting one
  wrongly would understate the loss.
- A charge to a venue account that names no order (a daily regulatory fee, a perpetual's funding
  payment) is part of the figure
  on the day it is recorded, by the same rule: paid and stated in an asset, or left out and
  counted.
- The limit is optional in configuration. A node without one has no automatic stop on loss.

## Drawdown

`drawdown` is declared at portfolio scope only, in money: how far the day's result stands below
the highest result the day has reached. The result is the same figure the daily loss is the
opposite of, fees included as above. The day opens at a result of zero, so a day that only lost
has drawn down by all of its loss, and a day that rose and fell back is judged from its high.

- Each new high is written to the control stream as `DayPeak` before the figure is judged, so a
  restart measures from the high already reached, not from the open.
- Without a result (no baseline, an unvalued book, a fill that cannot be priced) no figure is
  produced, and the limit is unjudged and halts the node.
- A drawdown against account equity is not built; the equity is read for leverage (decision 0011).

## Leverage and margin use

`leverage` and `margin_use` ([decision 0011](../decisions/0011-account-margin-and-leverage.md))
are declared at portfolio scope only, as ratios: `maximum_ratio_scaled`, where 10000 is one (30000
is three times). Leverage is the gross exposure over the equity the venues count as margin; margin
use is the venues' maintenance margin over that equity, where one is the point a venue may
liquidate. Both are rounded up.

- The equity and maintenance margin are what each account read states (Bybit `totalMarginBalance`
  and `totalMaintenanceMargin`, Alpaca `equity` and `maintenance_margin`), converted into the
  book's currency at declared rates.
- A venue read that states none (another venue, a Bybit account in isolated margin mode, a margin
  read that failed), equity at or below zero, a currency without a rate or an unvalued book leaves
  no figure: the limit is unjudged and halts the node.

## Underlying exposure

`gross_exposure` and `net_exposure` may be declared at `underlying:<asset>`: one asset across
every venue, whatever instruments hold it. The decision is recorded in
[0001](../decisions/0001-underlying-scope-level.md).

- An instrument belongs to the underlying its profile's `base_currency` names, compared without
  regard to letter case. A spot pair and a perpetual on one asset add into one figure.
- Venues write one asset under different names (PEPE is `PEPE` on Bybit spot, `1000PEPE` on Bybit
  linear and `kPEPE` on Hyperliquid). The configuration's `assets` table names the asset behind
  each ([decision 0006](../decisions/0006-asset-names.md)):
  `"assets": { "pepe": ["1000pepe", "kpepe"] }`. An asset traded under two names with an
  underlying limit declared on it must have both listed, or the limit sees one of them.
- One held position whose instrument declares no base produces no underlying figure at all, and
  an underlying no venue lists has none either. A declared underlying limit is then unjudged and
  the node halts.
- An admitted order draws its spend from its underlying's budget too, where its instrument
  declares a base. A breach at this level halts.

## Desk and strategy exposure

`gross_exposure` and `net_exposure` may be declared at `desk:<name>` and `strategy:<name>`. A
venue reports what it holds by instrument, never by who placed it, so these figures come from the
platform's own placement record: every intent names the desk its proposal named and the strategy
that made it, and a desk's or strategy's exposure is built from the recorded fills of the orders it
placed, valued at the same marks as the portfolio.

- **A strategy is named by the strategy engine.** The supervisor stamps every proposal with the
  name it runs the strategy under (`--name`, or the program's file name without its extension). A
  strategy cannot name itself or another. A `strategy:<name>` limit binds that name as written;
  a desk limit must name a declared desk, or the configuration is refused.
- **Every figure is a bound, never an estimate.** The position of an order that may still execute
  lies between all of it filling and none of it, and the figure takes the worse. Net exposure is
  the end of that range furthest from flat.
- **What the record does not explain counts against everyone.** Whatever a venue holds beyond the
  recorded fills (a fill not yet read by reconciliation, a position taken outside the platform, an
  adjustment that named no owner, a fill of an order recorded before intents named an owner) is charged in full to
  every desk and every strategy, because nothing says whose it is. So is an order no strategy is
  named on, at the strategy level. A node that holds positions it did not place therefore
  judges every desk and strategy against them until they are closed.
- **An adjustment may name its owner.** A risk officer who accepts a position no fill explains
  may name the desk, and the strategy, it belongs to (`bijective issue ... adjust-position <venue> <instrument> <side> <quantity> <price> --desk <desk> --strategy <name> <reason>`). It then counts
  as that owner's, like a fill, and no longer against everyone. A desk that is not declared is read
  as no owner, so a misspelt desk cannot leave the desk that was meant understated.
- **Measured whole or not at all.** No desk or strategy figure is produced when the book could not
  be valued, when the order record could not be rebuilt, or when an order or position cannot be
  priced (no mark, not catalogued, a currency with no rate into the book's). A declared desk or strategy limit
  is then unjudged, and an unjudged limit halts the node.
- **Pre-trade, the spend binds them too.** An admitted order draws its spend from the desk's and,
  where the proposal names one, the strategy's budget, as it does from the portfolio's.

## Desks and admission

A desk is a declared structure over the platform's holdings, not a second record of them.

- Desks may not be allocated more capital in total than the portfolio holds. An unallocated
  remainder is reported.
- With desks in use, an order naming no desk is refused, never placed on a default desk.
- **Admission.** A desk declares the instruments it may trade and its venue preference. The order's
  spend is computed as quantity times price at the instrument's own scales. An order above the
  desk's ceiling is refused, never resized.
- **Price collar** ([decision 0010](../decisions/0010-price-collar-and-flatten.md)). A limit
  price that crosses the instrument's mark by more than the desk's `price_collar_bps` (default
  500) is refused (`OutsidePriceCollar`). A price that does not cross is not bounded. Every
  order, market orders included, is refused with no mark or a mark older than the freshness
  bound (`NoMarkForCollar`, `MarkTooOldForCollar`): on a flat book no position carries the mark's
  age into the figures, so this is where a stale market stops a new order.
- **Stops** ([decision 0012](../decisions/0012-stop-orders.md)). A stop is checked against the
  price increment like a price. A stop the mark has already passed is refused
  (`StopAlreadyCrossed`), and a stop-limit's price is collared around its stop.
- **Venue eligibility is derived; preference is declared.** A venue is eligible when its catalogue
  lists the instrument, the instrument is trading and an execution side exists for the credential.
  A venue the desk did not declare is never used as a fallback.
- **A proposal may name its venue.** A decision made for one venue (an arbitrage leg, a venue's
  fee or liquidity) names it. It is admitted there or refused: a venue the desk does not route
  to is `VenueNotMandated`, and a named venue where the instrument is not eligible is
  `NoEligibleVenue`, never moved to the next venue in the preference. Unnamed, the preference
  decides.
- **Outcome shares** ([decision 0002](../decisions/0002-prediction-market-outcomes.md)) are traded
  and valued like a spot holding. A limit or cap at or beyond the payout bounds (`0` and one unit
  of collateral) is refused (`OutsideOutcomeBounds`).
- **Equities** ([decision 0004](../decisions/0004-multi-asset-equities.md)) are traded and valued
  like a spot holding, long only: a sell beyond the holding is refused before it is sent.
- **Instrument kinds.** The platform trades spot pairs, linear perpetuals, outcome shares and
  equities. An instrument a
  preferred venue lists as a dated future or an inverse contract is refused with its kind named
  (`KindNotTradable`), and a held position of such a kind is reported unvalued, which leaves the
  exposure snapshot empty and the limits unjudged. Neither is valued as if it were linear.
- Price and quantity are checked against the venue's increments before the order leaves.

## Authority

Operator actions are decided by an exhaustive role-action table. Roles are not ordered, so a new
action is not silently granted to senior roles; the compiler requires a decision for every pair.

| Action | Observer | Operator | Risk officer |
|---|---|---|---|
| halt, reduce, acknowledge-alarm, withdraw-venue | yes | yes | yes |
| resume, admit-venue, approve-recommendation | no | yes | yes |
| cancel-orders, flatten | no | yes | yes |
| change-limit, adjust-position | no | no | yes |

- Any role, including an observer, may halt, reduce, acknowledge an alarm and withdraw a venue:
  each only narrows what the platform may do.
- Resuming relaxes, and an observer may not take it.
- Changing a limit and adjusting a position belong only to the role that owns what may be risked,
  so the person who resumes is not the person who accepts what the record holds.

`Action::ALL` is the one list of actions; every record that names an action by name is read back
against it.

Authority decides what a stated role may do. It is not authentication; the next section is.

## Who is asking

A command names its operator and a role, and both are claims. The control engine believes a
command only when:

1. the name is in the operator registry (`--operators`, a JSON file of `{ name, role, public_key }`),
2. the command's Ed25519 signature verifies under that operator's public key,
3. the role the command claims is the role the registry holds for that name, and
4. its identifier has not been carried out before, so a recorded command appended again does
   nothing.

Any other command is recorded as refused, with the reason, and changes nothing. The signed
message is the command's canonical encoding without the signature: the verifier decodes the
command and encodes it again, so nothing is sliced from the wire.

Live mode refuses to start without a registry. Other modes may run without one, in which case the
claimed role stands, as before.

The `bijective` command line creates a key (`bijective keygen`, which refuses to replace an
existing key and writes the private half owner-only) and signs and appends a command
(`bijective issue`). The public half goes into the registry; the private half stays with the
operator.
