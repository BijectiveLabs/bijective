# Close positions when a node stops

Platform-Version: 0.1.0
Drill: crates/control_engine/tests/unit/path_tests.rs
Applies-To: any node trading live

A stopped node watches nothing. Its resting orders stay at the venue and can fill, and its positions
stay open. This runbook says what protects them and what the operator does.

## What the venue offers

| Venue | Protection that survives the node | Status |
|---|---|---|
| Bybit | Disconnect cancel all (`/v5/order/disconnected-cancel-all`): cancels active orders when every private connection subscribed to a `dcp` topic has been gone for the time window (3 to 300 s) | Granted per account by the venue's account manager to institutional clients only. Not used by the platform |
| Alpaca | None read | Not available |
| Hyperliquid, Polymarket | Not read yet | Not available |

Positions are not closed by a cancel. A reduce-only stop order
([decision 0012](../decisions/0012-stop-orders.md)) rests at the venue and survives the node where
the adapter places stops natively (Bybit and Alpaca). A position without one is unprotected while
the node is stopped, and so is every position on a venue whose adapter refuses stops.

## Steps

1. **Start the node again.** It starts halted, reconciles every venue, and its first pass withdraws
   every resting order that could add exposure. Reduce-only exits on a venue that enforces them are
   left working.
2. **Close what is held, if the stop was not planned:** `bijective issue <store> <operator> <role> <key> flatten`. The node places one reduce-only market order per position in its record, past the
   risk figures (decision 0010). Read the outcome under Audit on the console.
3. **If the node cannot start**, cancel the orders and close the positions at the venue by hand.
   The node then reads them as changes made outside the platform and halts on its next start until
   a risk officer adjusts the record (`adjust-position`).

## Before trading live

- Keep the node's host under a supervisor that restarts it (`install_and_start_a_node.md`).
- Size positions so that the loss over the time it takes to reach step 3 is one the desk accepts.
