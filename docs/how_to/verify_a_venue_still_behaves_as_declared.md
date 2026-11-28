# Verify a venue still behaves as declared

Platform-Version: 0.1.1
Drill: scripts/verify/venue_assurance.sh
Applies-To: before a venue is first traded, after any venue announcement, and on the cadence the
            venue's profile declares

A profile can be drafted from the same published rules with `bijective-profile <venue> <instrument,...> <url> --out <dir>`; the draft is unverified until this check confirms it.

## Why this is not a gate

It reaches an exchange. **A check that depends on an external service fails when that service has
an outage**, which is the moment it is least useful to be blind and most tempting to disable. So it
is a drill an operator runs, and the change gates stay runnable without a venue.

## Why it needs no credential

Both halves are public. Market data is public, and a venue's precision, tick, lot and funding
interval are things it tells everyone. **The execution half of the conformance suite is the part
that needs a key**, and it is a separate run.

## Steps

The endpoints are the venue's, and they come from **its profile** rather than from here. A guide
that wrote them down would drift from the profile the same way a hand-edited unit drifts from the
manifest, and the adapter contract keeps venue specifics out of everything above the contract - this file
included.

```
export BIJECTIVE_STREAM_ENDPOINT=...   # from the venue's profile
export BIJECTIVE_REQUEST_ENDPOINT=...
export BIJECTIVE_INSTRUMENT=...
bash scripts/verify/venue_assurance.sh
```

The script asks the registry which venue this build carries. **It refuses to guess an endpoint** -
a guessed endpoint is unverified, and a drill pointed at the wrong venue reports about a venue
we do not trade.

The assurance runs **first**, and its answer enters the suite rather than sitting beside it - the
suite used to report no result on `declaration-matches-live-behaviour` where an answer already
existed.

## Reading the outcome

| Line | Meaning |
|---|---|
| `PASS` | The adapter honoured that clause of the contract |
| `LIVE-NEEDED` | The check exists and has not been satisfied. **Not a pass** |
| `ASSURANCE  the venue behaves as the profile declares` | The declaration is still true |
| `ASSURANCE  the profile and the venue disagree` | Read the findings. `[capital-relevant]` means the risk limits may already be judging against a guarantee the venue never made |
| `releasable: true` | Every check answered and passed: the adapter is done in the conformance suite's sense |
| `releasable: false` | At least one check is unproven or failed. Unproven is not a failure and is not a release either |

## If the profile and the venue disagree

**Do not adjust the platform to match the venue and move on.** The divergence is the finding, and
the verifier has no path that could change the profile - it takes it by shared reference, which is
the design rather than a convenience.

The sequence is: read the finding, decide what the declaration should say, change the declaration
deliberately, and record why. A capital-relevant divergence holds trading on that venue
until it is resolved.

**This has already happened once, in the drill's own profile.** It declared a tick of `0.01` where
the venue enforces `0.10`, the verifier found it against live data and marked it capital-relevant,
and the declaration was corrected by hand. The correction is annotated in the source, because a
value that changed without a reason recorded is the next person's mystery.
