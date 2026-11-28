# Examples

`reference/` holds the reference strategies the platform is exercised with: `momentum.py`
(long-only time-series momentum on the minute mid) and `zscore_reversion.py` (mean reversion on
the minute mid). They exist to drive the order path, risk limits and reconciliation end to end,
not as trading advice.

```bash
bijective-backtest python/bijective-strategy/examples/reference/momentum.py
```
