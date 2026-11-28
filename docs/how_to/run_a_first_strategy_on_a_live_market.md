# Run a first strategy on a live market

Platform-Version: 0.1.1
Drill: scripts/verify/first_run.sh
Applies-To: a first paper run and backtest on a real public market, before any venue key exists

**Why:** a strategy should meet a real market before anyone writes a venue profile or holds a
key. Paper places no order at the venue, and the market data it records is public.

## Commands

The example uses the Bybit linear perpetual `BTCUSDT`. Any venue whose instrument rules this build
can read works the same way: Bybit and Hyperliquid in this build (`docs/integrations/`).

```bash
export PATH="$PWD/target/release:$PATH"

bijective-profile bybit BTCUSDT https://api.bybit.com --out setup
bijective-backtest --new first.py
bijective-paper first.py setup wss://stream.bybit.com/v5/public/linear --store store --minutes 10
bijective-backtest first.py --store store --market market-bybit --config setup/config.json
```

1. `bijective-profile` reads the venue's published instrument rules (tick, lot, minimums,
   currencies, funding) and writes `setup/profile.json` and a starter `setup/config.json`: one
   desk, a sandbox of 10,000 in the settlement currency and starting limits. Nothing in either is
   guessed; what the venue does not publish is refused or stated. The profile is unverified until
   [venue assurance](verify_a_venue_still_behaves_as_declared.md) checks it.
2. `--new` writes a strategy that buys at the ask and sells at the bid twenty updates later, so the
   order path has something to do. Replace its rule with yours.
3. `bijective-paper` records the live public market, runs the strategy in a paper session and
   places its proposals on the simulated venue through the same limits, order path and
   reconciliation as a live node. With no operator registry it resumes itself by a recorded
   command. At the end it prints orders, fills, holdings, refusals, cash and equity.
4. The backtest runs the same strategy over what step 3 recorded
   ([decision 0015](../decisions/0015-backtest-through-the-order-path.md)).

## Reading the two results

The two runs see the same market but are not the same run. In paper an order reaches the
simulated venue at the next pass, up to five seconds after the decision; in a backtest it meets
the book it was decided on. Paper tells the strategy of its fills; a backtest does not yet. A
difference between the two is a measure of that latency and that feedback, not a fault.

## Before live

The starter configuration's order rate, limits and sandbox balance are starting values. A live
node needs the venue's published order rate, the operator's limits, a venue key
([Load a venue credential](load_a_venue_credential.md)) and an operator registry for signed
commands ([Install a node and bring it up](install_and_start_a_node.md)).
