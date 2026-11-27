# bijective-event-store

Append-only, hash-chained segment store with a single writer per stream, and `projection`: rebuild-by-replay, where a projection that has not been rebuilt cannot be read.

See [Event sourcing](../../docs/concepts/event_sourcing.md).
