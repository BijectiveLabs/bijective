# bijective-strategy

The strategy protocol for Python strategies. Standard library only.

```python
from bijective_strategy import Book, EndOfWindow, answered, events, propose

for event in events():
    if isinstance(event, Book):
        if int(event.ask_price) - int(event.bid_price) < 200:
            propose(desk="alpha", instrument=event.instrument, side="buy",
                    quantity_scaled="1", price_scaled=event.bid_price,
                    client_id="c1", decided_at_ns="1")
        answered()
    elif isinstance(event, EndOfWindow):
        break
```

See [Strategies](../../docs/concepts/strategies.md).

## Reference strategies

`examples/reference/` holds strategies that exercise the platform end to end: the order path, the
risk gate and the record. They are not recommendations.

| File | Rule |
|---|---|
| `zscore_reversion.py` | Short above two standard deviations of the minute mid, long below, flat within half of one |
| `momentum.py` | Long one unit when the 15-minute move clears one standard deviation, sold after 15 minutes |

Each keeps its decision core free of input and output so it is tested without the platform, and
each runs as a set over one window: `bijective-backtest examples/reference/`.
