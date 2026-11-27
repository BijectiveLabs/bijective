# Capabilities

The platform is specified as seventeen capabilities. Each has a fixed scope and a test that shows
the core still runs without it where it is operational.

| Capability | Scope | State |
|---|---|---|
| Operator console | The view of one run in any mode (decision 0017): node state, limits, orders, fills, alarms and strategy performance as pages, tables, an event stream and OpenMetrics, on loopback or over HTTPS with client certificates, with signed control forms | Built |
| Missing-event detection | Declared expected events per integration and interval; undeclared reads as unmonitored | Expectation registry, the detections format and its console panels built. Evaluator planned |
| Deployment and operations | Manifest, unit generation, release artefact verification, status, restore drill | Built |
| Upgrade assurance | Compatibility asserted by test before a version change; one-step rollback | Planned |
| Configuration assurance | Refuse unknown keys, unset safety settings and limits no producer can evaluate | Built |
| Operational intelligence | Incident and quirk catalogue linked to the alerts it explains | Planned |
| Decision registry | Recorded decisions and pre-registrations, append-only and hash-linked | Pre-registration registry built |
| Venue assurance | Venue profiles, instrument catalogue, scheduled verification against the live venue | Built |
| Risk and limits | Limits at six scope levels (decision 0001), pre-trade check, safe states, durable limit state | Built |
| Identity, access and secrets | Role-action authority table, secrets broker, encrypted keystore | Built |
| Desks and capital | Desk register, capital allocation, order admission | Built |
| Audit reporting | Exportable evidence packs of decisions, orders and fills | Planned |
| Continuity and recovery | Backup verification, restore drill, interrupted-start recovery | Restore and interrupted-start drills built |
| Strategy promotion | Backtest, paper and live stages decided from recorded evidence | Backtest, paper and live built. Paper and backtest fills are simulated on the simulated venue. A backtest does not yet tell the strategy of its fills (decision 0015, stage 2) |
| Execution quality | Cost against the book at the decision, decomposed into spread, fees and funding | Decision capture and the console's execution cost table built. Attribution built in part |
| Research integrity | Pre-registration, walk-forward validation, sample sufficiency, cost floor | Built |
| Reconciliation | Platform record against the venue account, classified divergences | Built: on a per-venue interval and when a venue reports a change |

See [ROADMAP.md](../../ROADMAP.md) for order of work.
