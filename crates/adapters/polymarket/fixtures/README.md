# Fixtures

Recorded Polymarket traffic that the adapter's tests replay. `recorded/` holds frames exactly as the
venue sent them: `market_2026-11-15.jsonl` is the market channel on 2026-11-15. A fixture is
recorded, never written by hand, so a test checks the adapter against what the venue actually
sends.
