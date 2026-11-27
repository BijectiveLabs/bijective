# 0016: Market history from a client's file

| Field | Value |
|---|---|
| Status | Accepted, 2026-11-22. Stage 1 built |
| Extends | The market stream (`bijective_serialization::signal::market`), precision per instrument (decision 0007), the backtest (decision 0015) |

## Context

Market data entered the platform only as venue frames read from a socket. A client who already
holds history, bought from a vendor or exported from a venue, could not backtest on it. Nautilus
reads such files with a loader per format and a converter to its domain types at the instrument's
precision; LEAN keeps a fixed data format per security type with converters beside it.

## Decision

1. **A file becomes the market stream a recording writes.** `bijective-import` reads a CSV or
   JSON Lines file of top-of-book rows and appends them to `market-<venue>` of the venue the given
   profile names. Backtest, paper and the simulated venue read it as they read a recording; no
   second data path or catalogue is added.
2. **The layout is declared.** The canonical fields are `time`, `instrument`, `bid_price`,
   `bid_quantity`, `ask_price` and `ask_quantity`. `--columns` names the file's column for a field;
   `--instrument` stands for a file with one instrument and no instrument column. `--time-unit`
   (`ns`, `us`, `ms`, `s`) is required: a time unit is never inferred.
3. **Precision is the instrument's.** Values are scaled at the instrument's price and quantity
   scale from the profile. A value finer than that scale, an instrument the profile does not list,
   a time that goes back, a quoted CSV field or a JSON number with a fraction is refused with its
   line number. Prices and quantities in JSON are strings: a JSON number passes through a binary
   float.
4. **The whole file is checked first.** A refusal writes nothing. A store that already holds the
   venue's market is never added to, so importing a file into a fresh store writes the same bytes
   every time.
5. **Nothing is invented.** A file numbers nothing the venue published, so its books carry no
   sequence. It has no receipt time, so the venue time is the only time, at millisecond
   resolution. Each side is one level deep.

## Stage 2 (when a client needs it)

Trades, funding, mark prices, open interest and book deltas, which `MarketFact` already carries;
Parquet input through the `parquet` crate the archive uses; OHLCV bars, which have no bid or ask and
need a stated fill assumption and a bar event for strategies.

## Consequences

- An imported market has no frame journal and no gap rows: it is not a recording of a socket.
- Rows less than a millisecond apart share a venue time; their order is the file's.
- `scripts/verify/import_path.sh` imports a file twice to the same bytes, refuses a finer value by
  line, and fills a backtest on the result.
