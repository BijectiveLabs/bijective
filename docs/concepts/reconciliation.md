# Reconciliation

Reconciliation compares what the platform holds with what the venue reports, as of a stated
position in the event sequence. The venue is the source of truth for what happened there; the
platform's record is the source of truth for what it intended. Neither is overwritten by the other.

## Read-only on both sides

`bijective-reconciliation` places no order and writes no authoritative state. `PlatformPosition` and
`VenuePosition` are separate types with no conversion in either direction, so adopting the venue's
record does not compile. A test also refuses any module that can see a `VenuePosition` and
constructs a `PlatformPosition` from it.

A divergence is reported to an operator, who decides. A resolution records the person who made it.

## As of a sequence position

A comparison is made as of a position in the platform's event sequence, never "as of a time". A
wall-clock moment is ambiguous across streams; a sequence position is not.

## Classes of divergence

| Class | Response |
|---|---|
| In-flight | A window, recomputed every run. A divergence that outlives the window is reclassified rather than staying quiet |
| Missing | The platform holds a record the venue does not. Raised; the platform's record is never deleted |
| Extra | The venue holds something the platform did not see. Raised at the highest severity: capital moved and the order path did not know |
| Value | Both hold the record and a field differs. Raised with the field and both values shown; neither value is adopted |

A venue record that cannot be paired at the fidelity the profile declares is a profile defect, kept
in its own list so it is not reported as a position divergence.

## Confidence and age

- A pairing made through the profile's declared fallback key is labelled `Fallback`, never `Exact`.
- Severity follows age, not count: "four open" reads the same on day one and day thirty. An `Extra`
  starts at `Overdue`.

## Reading the venue's account

`VenuePosition::from_account` takes the adapter's account snapshot through
`VenueExecution::account`, which needs the adapter's execution side and a credential from the
secrets broker. A venue reports one net position per instrument and no order identifiers, so
position reconciliation pairs by instrument and labels those matches `Fallback`.

A position the adapter cannot read exactly is dropped rather than rounded, so reconciliation
reports it as `Missing` against the platform's record instead of reconciling clean against a book
the platform does not hold.

When and how often reconciliation runs is per-venue configuration.
