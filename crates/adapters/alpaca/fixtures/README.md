# Fixtures

Recorded Alpaca traffic that the adapter's tests replay. `recorded/` holds frames exactly as the
venue sent them: `test_stream_2026-11-17.jsonl` is the market data test stream on 2026-11-17. A
fixture is recorded, never written by hand, so a test checks the adapter against what the venue
actually sends.
