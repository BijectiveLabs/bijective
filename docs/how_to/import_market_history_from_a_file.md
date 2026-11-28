# Import market history from a file

Platform-Version: 0.1.1
Drill: scripts/verify/import_path.sh
Applies-To: backtesting on history a client already holds, as CSV or JSON Lines top-of-book rows

**Why:** history bought from a vendor or exported from a venue should run through the same
backtest and simulated venue as a recording, without a second data path
([decision 0016](../decisions/0016-market-history-from-files.md)).

## Steps

1. A profile for the venue the history is from, which gives each instrument its precision:
   `bijective-profile <venue> <instrument> <url> --out setup`
   ([Run a first strategy on a live market](run_a_first_strategy_on_a_live_market.md)).
2. Import the file into a store of its own:

   ```bash
   bijective-import history.csv --profile setup/profile.json --store history --time-unit ms \
       --columns time=ts,instrument=symbol,bid_price=bid,bid_quantity=bid_size,ask_price=ask,ask_quantity=ask_size
   ```

   A file whose columns already carry the canonical names (`time`, `instrument`, `bid_price`,
   `bid_quantity`, `ask_price`, `ask_quantity`) needs no `--columns`. A file with one instrument
   and no instrument column takes `--instrument <symbol>`. A `.jsonl` file holds one object per
   line with the same names; give prices and quantities as strings.
3. Backtest on it:

   ```bash
   bijective-backtest first.py --store history --market market-<venue> --config setup/config.json
   ```

## Refusals

Every refusal names the line, and nothing is written:

| Message | Fix |
|---|---|
| `line N: bid_price: ... precision` | The value is finer than the instrument's tick scale. Check the profile matches the file's venue |
| `line N: ... is not in the venue's profile` | Add the instrument to the profile, or name the file's instrument with `--instrument` |
| `line N: time ... is before the row above it` | Sort the file by time |
| `line N: quoted fields are not read` | Export without quotes |
| `line N: ... is a JSON number` | Write prices and quantities as strings |
| `... already holds market-<venue>` | Import into a new store; a store's market is never added to |

The unit of the time column is never guessed: `--time-unit` is `ns`, `us`, `ms` or `s`.
