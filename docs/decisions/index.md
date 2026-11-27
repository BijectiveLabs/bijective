# Decision records

A decision that changes what the platform measures, records or refuses is written here before
the code that carries it out.

| Record | Decision |
|---|---|
| [0001](0001-underlying-scope-level.md) | A sixth limit level, `underlying`, binding one asset across venues |
| [0002](0002-prediction-market-outcomes.md) | Prediction-market outcome shares as a tradable instrument kind |
| [0003](0003-sandbox.md) | The sandbox: a paper node with a simulated account, fees and balances |
| [0004](0004-multi-asset-equities.md) | Multi-asset scope; US equities, long only, through Alpaca first |
| [0005](0005-reporting-currency.md) | One reporting currency per node, converted at rates read from declared markets |
| [0006](0006-asset-names.md) | One asset, many venue names: an operator table read wherever a profile is read |
| [0007](0007-precision-per-instrument.md) | Precision per instrument, read from the profile; venue-wide scales retired |
| [0008](0008-accounts-apart-from-venues.md) | An account is named apart from its venue: several accounts at one venue in one node |
| [0009](0009-time-in-force-and-post-only.md) | Time in force and post-only, stated by the strategy and honoured or refused by each venue |
| [0010](0010-price-collar-and-flatten.md) | A price collar around the mark at admission; `flatten` closes what the record holds past the figures |
| [0011](0011-account-margin-and-leverage.md) | Account margin read from the venue; `leverage` and `margin_use` judged for the whole book |
| [0012](0012-stop-orders.md) | Stop and stop-limit orders held natively at the venue; a strategy may propose reduce-only orders |
| [0013](0013-linked-orders.md) | Linked orders held by the platform: a child released on its parent's fills, one-cancels-other groups |
| [0014](0014-expiry-at-a-time.md) | Expiry at a time, an order term the node's sweep carries out |
| [0015](0015-backtest-through-the-order-path.md) | A backtest runs the order path on simulated venues, its passes placed by the recording |
| [0016](0016-market-history-from-files.md) | Market history from a client's CSV or JSON Lines file becomes the market stream a recording writes |
| [0017](0017-one-console-for-every-run-mode.md) | One console for every run mode: research and trading are the same views of different runs |
| [0018](0018-paper-venue-rebuilt-from-its-record.md) | A paper venue is rebuilt from its record at start, so a restarted paper run continues |
| [0019](0019-market-journal-durable-per-batch.md) | The market journal is made durable per batch of what is already waiting, never per frame |
| [0020](0020-mcp-server-for-research-agents.md) | Proposed: an MCP server through which a research agent runs the research programs, with no route to an order |
