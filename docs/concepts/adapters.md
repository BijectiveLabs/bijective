# Adapters

Each venue is a protocol: its own authentication, rate limits, order types, error codes, wire format
and reconnect behaviour. An adapter translates one venue's protocol into the platform's model.
Nothing above the adapter contract names a venue or sees a venue format.

| Crate | Role |
|---|---|
| `bijective-venue::contract` | The traits every adapter implements, and the conformance suite |
| `bijective-venue::normalise` | Venue decimal strings to scaled integers; the normalised outcome taxonomy |
| `bijective-venue::profile` | The venue profile, the instrument catalogue and the capability declaration |
| `bijective-venue-registry` | Which venues this build carries, and how one is opened |
| `bijective-venue-alpaca`, `bijective-venue-bybit`, `bijective-venue-hyperliquid`, `bijective-venue-polymarket` | One crate per venue |
| `bijective-venue-simulated` | A venue double that fills: a deterministic matching engine behind the execution contract ([Simulated venue](simulated_venue.md)) |

## Target venue portfolio

The professional target portfolio is **Binance, Bybit, Hyperliquid, OKX, Polymarket, Bitget,
Kalshi, Alpaca and Interactive Brokers** ([decision 0004](../decisions/0004-multi-asset-equities.md)).
The target list is intentionally broader than the currently implemented crates. A venue
does not become supported merely because it appears in this list: it must pass the adapter contract,
official-documentation evidence, fixture replay, conformance, and live assurance requirements below.

The current implementation status is maintained in the
[integration index](../integrations/index.md). No strategy, risk, or execution component may
depend on the target list; venue selection remains a registry and adapter concern.

## The contract

Market data and execution are separate traits, `VenueData` and `VenueExecution`. They have
different lifecycles, failure modes and credentials: a read-only key can serve data with no
execution capability at all. `VenueFactory` builds both and is the only place a venue name appears
in a wiring path.

The types make four mistakes unrepresentable:

- **Sending is not attaching.** `SubscriptionAck` is `Confirmed` or `Unconfirmed`. A subscription
  the venue has not confirmed is not a subscription.
- **A degraded connection is not usable.** `ConnectionState` has four values: `Disconnected`,
  `Connecting`, `Connected` and `Degraded` (backlogged, stale, or with subscriptions incomplete).
  An attached but stale connection is worse than a down one, because it looks calm.
- **A reconnect declares its continuity.** `Reconnected` carries a `Continuity`: `Unbroken`, a
  bounded `Gap`, or `GapOfUnknownExtent`. Reconnecting is not resuming; books are dropped across a
  gap and rebuilt from a snapshot.
- **Submitting returns an outcome, not a `Result`.** "We could not tell" is one of the answers.

`VenueExecution::submit` requires a `Clearance`, which only a passing risk evaluation can mint.
`cancel` requires none, because a cancel only reduces exposure.

## Market records

A frame from a venue becomes zero or more `MarketRecord` values and, where it touched a book with
both sides, one `BookTop`. `MarketFeed::offer` takes the frame and its arrival time and returns
both as an `Offered`; a live stream forwards each record as `MarketEvent::Record`, then the top
as `MarketEvent::Observation`.

Every record carries two instants:

| Field | Unit | Source |
|---|---|---|
| `exchange_time` | UTC nanoseconds, or `NotPublished` | The venue's own stamp, widened exactly from milliseconds |
| `received_at_ns` | UTC nanoseconds | The platform clock as the frame left the socket, before parsing; the recorded arrival time on replay |

The exchange time orders events within one venue. The arrival time decides what was knowable at
a moment, so a point-in-time state needs both.

| Fact | Data class | Content |
|---|---|---|
| `BookSnapshot` | `depth` | Both sides, best first, no zero-quantity level, with the venue sequence or `NotNumbered` |
| `BookDelta` | `depth` | `Set` and `Remove` changes; a zero quantity on the wire is written as `Remove` |
| `Trade` | `trades` | Venue trade identity (for deduplication), aggressor side or `NotPublished`, price, base quantity |
| `Liquidation` | `liquidations` | Side of the forced order, price, base quantity |
| `Funding` | `funding` | Signed rate per interval with its own scale (positive: longs pay shorts), next funding time |
| `OpenInterest` | `open-interest` | Base quantity or quote value, as the venue publishes it; never converted |
| `MarkPrice`, `IndexPrice` | `mark-price`, `index-price` | One scaled price |
| `LongShortRatio` | `long-short-ratio` | Long and short account shares with their scale, and the venue period |
| `Resolution` | `resolution` | Whether a prediction-market outcome won when its question resolved (decision 0002, rule 5) |

`check_record` is the record contract. The adapter applies it before a record leaves the feed, and
a record that fails is refused as `FeedError::RecordRefused` with the field and value. The
conformance function `check_feed` replays recorded frames through a feed and fails when a frame is
refused, when no record is produced, when a record breaks the contract, when a record carries an
arrival time other than its frame's, or when a record belongs to a class the declaration does not
serve.

Each frame is also handed up unparsed, as `MarketEvent::Received`, before anything parsed from it,
so the data engine journals it first ([Market archive](market_archive.md)). Today both adapters
emit book records only: Bybit snapshots and deltas from `orderbook.50`, and Hyperliquid snapshots
from `l2Book`. The other facts are part of the contract and have no producer and no archive table
yet; the archive refuses them by name until both exist.

## Normalisation

`bijective-venue::normalise` converts venue decimal strings to platform scaled integers. No path
uses a floating-point value.

- **Excess precision is refused, never rounded.** A value with more decimal places than the
  profile declares is a profile defect, reported with both numbers. Trailing zeros are not excess
  precision.
- `NaN`, infinities, malformed numbers and values wider than `i128` are refused. Exponent notation
  is accepted and applied by moving the decimal point in the digit string.
- **The scale is data**, read from the profile, never hard-coded.

Venue errors terminate at the adapter. Above it there are normalised outcomes:

- **Unknown is never a rejection.** A timeout, a dropped connection or an unreadable response does
  not settle whether an order reached the venue. Every unknown is routed to reconciliation; the
  order is not resubmitted under a new identifier.
- **An unrecognised venue code is carried, never mapped** to the nearest known rejection.
- **A duplicate is an answer.** The venue reporting a client order identifier it already holds
  confirms the first attempt landed, which is what makes resubmitting the same identifier safe.

## Profiles and the instrument catalogue

The venue profile is data an operator edits, and the adapter reads it. The profile crate depends
on no adapter, so a profile cannot be a description an adapter writes about itself.

- **Precision is per instrument.** Price tick, quantity step, minimum size and minimum notional are
  declared for each instrument. There is no venue-wide precision.
- **`VenueSymbol` and `InstrumentId` are different types.** The symbol goes on the wire; the
  platform identity is what risk scopes and portfolios are keyed by. One identity may carry symbols
  on several venues. The mapping is declared, never inferred from similar names.
- **Kind and status are declared.** Perpetual, spot, dated future and inverse instruments settle
  differently, and an instrument that is not trading is refused before the venue is asked.
- **Funding has an explicit model.** An absent funding interval cannot be encoded without choosing
  what the absence means, for example continuous funding versus an interval this transport does not
  publish.
- **The capability declaration is derived** from the profile (`from_profile` is its only
  constructor). An order type a venue emulates reads as unsupported. The declaration reports the
  tightest staleness bound across transports.
- **Wide integers travel as strings** in the JSON profile, because a JSON number is a double in most
  parsers.

See `crates/deployment/profiles/example.json` for a complete profile.

## Conformance

The conformance suite is written once against the contract and run against every adapter. It
checks that a subscription is acknowledged, that a reconnect declares its continuity, that a
degraded state is never reported as usable, that resubmitting one client order identifier does not
create a second order, and that an unknown outcome never claims the order did not happen.

A check that needs a live venue reports `RequiresLiveVenue`, which is not a pass: `is_releasable`
refuses while any check has not run. The suite's own tests include a deliberately faulty adapter
that each check must catch.

## Venue assurance

A venue changes without notice. `bijective-verify-venue` runs on a schedule and compares what the
venue publishes with its profile.

- It never corrects the profile. Adopting an observed value would erase the evidence that the venue
  changed.
- Its findings distinguish observed from unobserved fields, a venue that could not be reached from
  one that passed, and a profile defect from a change in venue behaviour. Each divergence states
  whether it is capital-relevant.
- A fee schedule may only be declared for a venue whose latest check is a confirmation.

The check is operational: with it stopped, the core runs on the published profiles.

## Retry, circuit breaker and request budget

`bijective-venue::resilience` holds what every adapter needs to survive an unreliable venue, so each
adapter does not write its own. Each piece is a pure value: time is passed in as nanoseconds and
randomness is a trait the caller supplies, so a session stays a function of its inputs.

| Piece | What it answers | Behaviour |
|---|---|---|
| `Backoff` | How long before the next attempt | The base doubled per consecutive failure up to a ceiling, then the wait is drawn from the upper half of that range. The lower half is kept so a retry always backs off, and the draw keeps clients from retrying in step. Safe for any failure count |
| `Jitter` | Where the draw comes from | `SplitMix64` seeded from the clock and the process number in production, `Fixed` in a test. A session built without one cannot be made outside a test |
| `CircuitBreaker` | When to stop calling a venue that keeps failing | Closed, open or half open. Consecutive failures open it; an open breaker refuses until its period has passed, then lets exactly one probe through; a probe that never reports is replaced after a period. A result that arrives late for a call made before it opened changes nothing |
| `RequestBudget` | How fast to ask | A quota of requests per period with a burst. A refused request spends nothing. The venue's own `Retry-After` outranks the local quota, and a shorter statement never shortens a longer one |
| `CallGate` | What a REST client applies to its own calls | A circuit breaker per path (submit, read), a request quota for reads, and the venue's `Retry-After`. A call that is held back never reaches the venue; a held submit is answered `NotSent`, a platform answer that closes the order without venue evidence. **A cancel is never held**: an exit must always be attempted. Submits are not charged to the read quota because the control engine bounds them by the venue's order rate. A throttled answer (429) holds reads but never opens a breaker, because the venue is alive and asking for patience |

Every venue adapter's reconnect sessions use `Backoff`, and every adapter's REST client routes each
call through one `CallGate`. A venue refusal such as insufficient margin is an answer and counts as
healthy; no answer or a server error counts against the breaker. The numbers (five consecutive
failures, thirty seconds open) are the platform's choice and each adapter declares its own read
quota from its venue's published limits.

## Adding a venue

1. Read the venue's official documentation and record each claim the adapter relies on, with its
   URL and the date read, in `docs/integrations/<venue>.md`.
2. Add `crates/adapters/<venue>` implementing the contract in `bijective-venue`, with the session written
   as a pure state machine and the socket in a separate transport module.
3. Record live traffic as fixtures and replay them in tests.
4. Register the venue in `bijective-venue-registry` and declare the crate in `scripts/ci/areas.txt`.
5. Run the conformance suite against the live venue before declaring execution.
