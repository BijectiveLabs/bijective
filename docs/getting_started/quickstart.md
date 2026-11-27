# Quickstart

Write a strategy, run it against a sample market inside the sandbox, and ask where it stands on the
path to capital. Nothing is installed and no venue is needed.

## Prepare

```bash
export PATH="$PWD/target/release:$PATH"
```

Runs are bounded by cgroup v2. Where `BIJECTIVE_CGROUP_ROOT` names a delegated subtree, it is used;
otherwise the command asks the init system for a user scope and says which subtree it created. It
never adopts a subtree delegated to another process.

## Run

```bash
bijective-backtest --new alpha.py
bijective-backtest alpha.py
bijective-promote store alpha.py 19600
```

1. `--new` writes a strategy that already runs, seeds a sample market so no venue is needed, and
   writes `bijective.json` with the store, market and settings, so the next command needs no
   arguments beyond the strategy. `--new alpha.sh` writes a shell strategy instead.
2. The second command runs the strategy in the sandbox and prints every choice it made: the market,
   the window, the exchange, the library it declared and the supervisor command it ran.
3. `bijective-promote` reads the record and reports where the strategy stands on the
   [promotion stages](../concepts/evaluation.md), and what to do next. `19600` is the critical
   value the evidence must clear, scaled by 10,000: 1.96, a two-sided 95 percent level. The
   platform has no default; the operator chooses it.

To run many strategies over the same window, name a directory: `bijective-backtest strategies/`
runs every `.py` file in it, each in its own sandbox and each recorded under its file name.

## What the result says, and what it does not

`bijective-report store alpha.py 19600` prints one page about the strategy: its runs, the claim it
declared, and what was measured.

- A backtest records every proposal the strategy made and prices it against the recorded market at
  the moment it was made: the spread it would cross, and the mid-to-mid return over the horizon its
  claim declared. A proposal resting behind the touch is kept on the record but not counted as
  marketable. The generated strategy buys at the ask and sells at the bid twenty updates later,
  so it crosses the spread both ways.
- Given a node configuration (`--config <file>`, or `"config"` in `bijective.json`), a backtest
  also places the proposals on [simulated venues](../concepts/simulated_venue.md) through the same
  risk limits, order path and reconciliation as live trading, and prints orders, fills,
  positions, refusals, the change in equity and the largest drawdown. The configuration names
  a venue the build registers, so the sample market is run without it. The strategy is not yet
  told of its fills in a backtest; a strategy that acts on them is measured in a paper run.
- A single run proves nothing on its own: `bijective-promote` asks for the run to be repeated and
  compared before the strategy can take its first step.

## A live market

[Run a first strategy on a live market](../how_to/run_a_first_strategy_on_a_live_market.md):
draft a profile from the venue's published rules, run the strategy in paper on the live public
market, and backtest it over what that run recorded. No key is needed.

## The sample market

The sample market is a generated book on a stream named `market-sample`, and the run's ledger entry
records that name. It is not a venue recording; no claim measured on it says anything about a real
market, and it is never added to a store that already holds one.

## Next

- [Strategies](../concepts/strategies.md): the protocol and the sandbox.
- [Evaluation and promotion](../concepts/evaluation.md): pre-registered claims and the promotion stages.
- [Install a node and bring it up](../how_to/install_and_start_a_node.md).
