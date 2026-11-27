# Evaluation and Promotion

A strategy moves towards capital one stage at a time, and each step is decided from the record the
platform already holds. The crates here decide; they never start a strategy or place an order.

| Crate | Role |
|---|---|
| `bijective-evaluation::evidence` | Pre-registered claims, out-of-sample enforcement, sample sufficiency |
| `bijective-evaluation::measure` | What a run's proposals cost before they earn anything, and their return over the declared horizon |
| `bijective-evaluation::promotion` | Whether a strategy may take the next stage |
| `bijective-paper-compare` | What two runs decided, and where they differ |

## Pre-registration

A claim is registered before the run it is about. A decision rule is a measure, a direction and a
threshold, so it can be applied mechanically rather than read as prose.

"Before" is established by content, not by timestamp: the registration is bound by digest to the
run's specification and window, so it cannot be dated early or carried to a different stretch of
history. Figures a run produced that the registration never named are reported and labelled
`Unregistered`.

## Out of sample

A walk-forward split is validated at construction. It refuses:

- no folds, so nothing was held out;
- a segment that reads nothing;
- a test segment that begins before the learn segment it follows ends;
- two test segments covering the same stretch of history;
- a fold reaching past the position the run was allowed to read.

Learn segments may overlap; each fold learning from everything before it is what walk-forward means.

## Sufficiency

Evidence is one of three answers:

- **Sufficient.**
- **Insufficient**, with the shortfall: an underpowered result says nothing about the hypothesis,
  so it is neither a pass nor a rejection.
- **Unknowable**, with the cause.

The required sample size is computed in integers and every division rounds up, so the requirement
is never understated. The confidence level is a parameter: how much evidence is enough is the
operator's decision about their own capital.

## Cost before return

`bijective-evaluation::measure` prices a run's proposals against the platform's own record of the market at the
decision, not against anything the proposal asserts.

- A limit order resting behind the touch may never fill. It stays on the record but is not counted
  as marketable.
- Every rounding goes against the strategy: spread, mean and deviation all round up. Figures are in
  basis points at the platform's fixed scale.
- The cost floor is the spread crossed twice plus the taker fee twice. The fee comes from the
  venue's declared, verified fee schedule.
- **What is unavailable is said, never defaulted.** A floor missing its fee is `Unavailable`, not a
  smaller floor.
- The return is marked mid to mid over the horizon the registration declared, signed by the side
  taken. A horizon reaching past the recorded market marks nothing rather than a shorter hold.

## The promotion stages

A promotion is a conjunction of answers computed elsewhere. Every term must hold:

| Term | Source |
|---|---|
| The run reproduces | Replay comparison |
| Its output is usable | The run ended cleanly |
| A claim was registered for this run | Pre-registration |
| The claim was met | The registered decision rule |
| It was measured out of sample | Walk-forward |
| The evidence covers the claim | Sufficiency |

Reproduction is checked first, because a run that cannot be repeated makes every later term a
statement about something that happened once. A refusal names the term that stopped it.

A cleared stage carries the run it was decided about, so skipping a stage cannot be expressed. The
paper stage runs a strategy against live data with no order reaching a venue (`--stage paper`);
its orders are placed and filled on the [simulated venue](simulated_venue.md).

```bash
bijective-promote <store> <program> <confidence>
```

The command finds every run of the program, walks the promotion stages as far as it goes, and turns the term
that stopped it into the next thing to do. `bijective-report` prints one page about a strategy from
the same record.

## Paper and live

`bijective-paper-compare` compares two runs over their decisions, not over instants: a paper run
and a live run never see the same market moment. Each recorded decision carries the market position
it was made from, and the comparison pairs decisions on it.

- Everything is compared except the instant, including client order identifiers.
- Two runs that decided nothing, or that cover different positions, are reported as having nothing
  to compare. That is a third answer, not agreement.
- A position answered by one run and not the other is a divergence.
- It does not judge whether a divergence was expected.
