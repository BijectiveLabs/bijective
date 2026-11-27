#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Verifies the guided path: an operator names a program, the platform walks the promotion stages from its record and names the next action.
# Drives real runs, so it needs a cgroup v2 subtree: systemd-run --user --scope --setenv=BIJECTIVE_CGROUP_ROOT=<subtree> -- ./scripts/verify/guided_path.sh
# Exit codes: 0 the guided path holds, 1 it does not.

set -uo pipefail

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

if ! command -v cargo >/dev/null 2>&1; then
    say "FAIL  no cargo on PATH, so the guided path is UNVERIFIED"
    exit 1
fi
for target in crates/serialization crates/strategy_engine crates/evaluation; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline --examples 2>&1)"; then
        say "FAIL  ${target} did not build"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done
# Built here, not assumed: a stale binary in the target directory is how a rejection passes.
for target in crates/evaluation crates/strategy_engine; do
    if ! out="$(cd "${CORE}/${target}" && cargo build --quiet --offline 2>&1)"; then
        say "FAIL  ${target} did not build"
        printf '%s\n' "$out" | tail -3
        exit 1
    fi
done

SUPERVISOR="${CORE}/target/debug/bijective-strategy-engine"
PROMOTE="${CORE}/target/debug/bijective-promote"
BACKTEST="${CORE}/target/debug/bijective-backtest"
[ -x "$SUPERVISOR" ] && [ -x "$PROMOTE" ] && [ -x "$BACKTEST" ] \
    || { say "FAIL  a binary is missing after a successful build"; exit 1; }

mkdir -p "${WORK}/store" "${WORK}/run"

CGROUP_ROOT="${BIJECTIVE_CGROUP_ROOT:-}"
if [ -z "$CGROUP_ROOT" ]; then
    say "NOT VERIFIED  set BIJECTIVE_CGROUP_ROOT to a cgroup v2 subtree delegated to this user;"
    say "              this drill will not guess one, and the guided path is unproven until it runs"
    exit 1
fi
CGROUP="${CGROUP_ROOT}/bijective-guided-$$"
mkdir "$CGROUP" 2>/dev/null || { say "FAIL  cannot create a cgroup under ${CGROUP_ROOT}"; exit 1; }
trap 'rm -rf "${WORK}"; rmdir "${CGROUP}" 2>/dev/null || true' EXIT
printf '%s' "+memory +pids" > "${CGROUP}/cgroup.subtree_control" 2>/dev/null || {
    say "FAIL  ${CGROUP_ROOT} does not delegate the memory and pids controllers"; exit 1; }

# One proposal per event, each quoting the event it was decided on, so every decision is priced against its own book.
cat > "${WORK}/steady.sh" <<'STRATEGY'
#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *end_of_window*) break ;;
    *'"event":"book"'*)
      at=$(printf '%s' "$line" | sed 's/.*"position":"\([0-9]*\)".*/\1/')
      printf '{"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"3000000","client_id":"c1","decided_at_ns":"1","decided_at_position":"%s"}\n' "$at"
      printf '{"emit":"answered"}\n'
      ;;
  esac
done
STRATEGY
chmod +x "${WORK}/steady.sh"

promote() { "$PROMOTE" "${WORK}/store" "${WORK}/steady.sh" 19600 2>&1; }
bounded() { # [exchange] [strategy]
    "$SUPERVISOR" --readiness "${WORK}/run/r.json" --cgroup-root "${CGROUP}" \
        --store "${WORK}/store" --strategy "${2:-${WORK}/steady.sh}" \
        --market "market-drill" --to-position 3 --exchange "${1:-stepwise}" >/dev/null 2>&1
}

nothing="$(promote)"
say "P0    $(printf '%s' "$nothing" | sed -n 2p)"
if printf '%s' "$nothing" | grep -q 'nothing recorded'; then
    say "PASS  a strategy nothing has run says so, and says what to run"
else
    fail "an unrun strategy was not reported as one: ${nothing}"
fi

# Widening spreads: a claim's evidence is sized on spread dispersion, and a flat spread has none.
(cd "${CORE}/crates/serialization" && cargo run --quiet --offline --example seed_market -- "${WORK}/store" "btc-perp" 6 50 >/dev/null 2>&1)
bounded
once="$(promote)"
say "P1    $(printf '%s' "$once" | sed -n 2p)"
if printf '%s' "$once" | grep -q 'backtest: the run was never replayed'; then
    say "PASS  the promotion stages is walked from the record and stops at the first term"
else
    fail "the first term was not the one reported: ${once}"
fi
if printf '%s' "$once" | grep -q 'next:.*run the same specification again'; then
    say "PASS  and the term is named with the next thing to do about it"
else
    fail "no next action was given: ${once}"
fi

bounded
twice="$(promote)"
say "P2    $(printf '%s' "$twice" | sed -n 2p)"
if printf '%s' "$twice" | grep -q 'Unregistered\|declare a claim'; then
    say "PASS  a reproduced run moves the answer on to the unregistered claim"
else
    fail "the answer did not move on after the replay: ${twice}"
fi

(cd "${CORE}/crates/evaluation" && cargo run --quiet --offline --example declare_claim -- \
    "${WORK}/store" "${WORK}/steady.sh" 3:stepwise "the spread it crosses stays under half a basis point" \
    spread_bps at-most 12000 not-held 0:2:2:4 >/dev/null 2>&1)
claimed="$(promote)"
say "P3    $(printf '%s' "$claimed" | sed -n 2p)"
if printf '%s' "$claimed" | grep -qv 'Unregistered'; then
    say "PASS  a claim declared before the run is the one the answer is about"
else
    fail "the claim was not read: ${claimed}"
fi

if printf '%s' "$claimed" | grep -q '  next:'; then
    say "PASS  whatever stops it, the answer carries an action"
else
    fail "an answer arrived with no action: ${claimed}"
fi

if printf '%s' "$claimed" | grep -q 'backtest: cleared'; then
    say "PASS  a strategy that names the event it decided on clears the first stage"
else
    say "      $(printf '%s' "$claimed" | tail -2)"
    fail "the backtest did not clear: ${claimed}"
fi

if printf '%s' "$claimed" | grep -qE 'paper: +no session'; then
    say "PASS  and the answer moves to the stage it has not reached, with the command for it"
else
    fail "the promotion stages did not move on after the backtest cleared: ${claimed}"
fi

# Fed whole, the same strategy cannot place a decision, so every line is refused with the reason.
# Copied under its own name so this run does not join the history of the strategy under test.
cp "${WORK}/steady.sh" "${WORK}/claims.sh"
whole_claims="$("$SUPERVISOR" --readiness "${WORK}/run/r.json" --cgroup-root "${CGROUP}" \
    --store "${WORK}/store" --strategy "${WORK}/claims.sh" \
    --market "market-drill" --to-position 3 --exchange whole 2>&1)"
say "P4a   $(printf '%s' "$whole_claims" | tail -1)"
if printf '%s' "$whole_claims" | grep -q 'sent its window whole'; then
    say "PASS  a claim to have placed a decision is refused by a run that cannot place one"
else
    fail "a whole exchange accepted a decision position: ${whole_claims}"
fi

# No decision event named: proposals price at the window's end, so the evidence cannot be sized.
cat > "${WORK}/silent.sh" <<'STRATEGY'
#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *end_of_window*) break ;;
  esac
done
printf '{"emit":"proposal","desk":"alpha","instrument":"btc-perp","side":"buy","quantity_scaled":"1","price_scaled":"3000000","client_id":"c1","decided_at_ns":"1"}\n'
STRATEGY
chmod +x "${WORK}/silent.sh"
for _ in 1 2; do
    bounded whole "${WORK}/silent.sh"
done
(cd "${CORE}/crates/evaluation" && cargo run --quiet --offline --example declare_claim -- \
    "${WORK}/store" "${WORK}/silent.sh" 3:whole "the spread it crosses stays under one and a fifth basis points" \
    spread_bps at-most 12000 not-held 0:2:2:4 >/dev/null 2>&1)
silent="$("$PROMOTE" "${WORK}/store" "${WORK}/silent.sh" 19600 2>&1)"
say "P4    $(printf '%s' "$silent" | sed -n 2p)"

if printf '%s' "$silent" | grep -q "priced at its window's end"; then
    say "PASS  one that does not cannot be sized, and the action names why"
else
    say "      $(printf '%s' "$silent" | tail -2)"
    fail "the action did not name why the evidence could not be sized: ${silent}"
fi

# 5: paper stage. The claim is declared against the session's own identity (class and window are inside it).
# The session window must outlast its seeding: a cargo run overran it under load, so the built seeder runs with room.
"$SUPERVISOR" --readiness "${WORK}/run/r.json" --cgroup-root "${CGROUP}" \
    --store "${WORK}/store" --strategy "${WORK}/steady.sh" \
    --market "market-drill" --stage paper --drill-seconds 8 >/dev/null 2>&1 &
sleep 2
"${CORE}/target/debug/examples/seed_market" "${WORK}/store" "btc-perp" 4 90 >/dev/null 2>&1
wait

(cd "${CORE}/crates/evaluation" && cargo run --quiet --offline --example declare_claim -- \
    "${WORK}/store" "${WORK}/steady.sh" live "the spread it crosses live stays under three basis points" \
    spread_bps at-most 30000 not-held 0:6:6:40 >/dev/null 2>&1)

walked="$(promote)"
say "P5    $(printf '%s' "$walked" | sed -n 3p)"
if printf '%s' "$walked" | grep -qE 'paper: +cleared'; then
    say "PASS  the paper stage clears after the backtest, for the same strategy"
else
    say "      $(printf '%s' "$walked" | tail -3)"
    fail "the promotion stages did not reach its end: ${walked}"
fi

if printf '%s' "$walked" | grep -qE 'live: +ready to be decided'; then
    say "PASS  and the platform says what is left, without starting anything"
else
    fail "the end of the promotion stages was not reported: ${walked}"
fi

# 5b: a clearance does not cross a change of bytes. Identical output hides it from reproduction.
printf '\n# the same behaviour, a different version\n' >> "${WORK}/steady.sh"
"$SUPERVISOR" --readiness "${WORK}/run/r.json" --cgroup-root "${CGROUP}" \
    --store "${WORK}/store" --strategy "${WORK}/steady.sh" \
    --market "market-drill" --stage paper --drill-seconds 8 >/dev/null 2>&1 &
sleep 2
"${CORE}/target/debug/examples/seed_market" "${WORK}/store" "btc-perp" 4 90 >/dev/null 2>&1
wait

changed_version="$(promote)"
if printf '%s' "$changed_version" | grep -q 'cleared a different version' \
    && printf '%s' "$changed_version" | grep -q 'run the earlier stage again on this version'; then
    say "PASS  a clearance does not carry to a version the earlier stage never measured"
else
    say "      $(printf '%s' "$changed_version" | tail -3)"
    fail "the promotion stages carried a clearance across a change of bytes: ${changed_version}"
fi

# 6: starting with no flags: the command picks conservative defaults and prints every choice.
first_run="${WORK}/first"
mkdir -p "$first_run"
starter="${first_run}/first_strategy.sh"

if "$BACKTEST" --new "$starter" >/dev/null 2>&1 && [ -x "$starter" ]; then
    say "PASS  one command writes a strategy, and it is executable"
else
    fail "the scaffold did not write a runnable strategy at ${starter}"
fi

# Own store, so what it finds is what it was given.
mkdir -p "${first_run}/store"
"${CORE}/target/debug/examples/seed_market" "${first_run}/store" "btc-perp" 5 50 >/dev/null 2>&1
started="$(cd "$first_run" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "$BACKTEST" "$starter" 2>&1)"

# Named separately: a run that admitted nothing still prints a market and a window.
if printf '%s' "$started" | grep -q 'market:   market-drill' \
    && printf '%s' "$started" | grep -q 'window:   0 to 4' \
    && printf '%s' "$started" | grep -q 'exchange: stepwise'; then
    say "PASS  it found the market, the window and an exchange without being told any of them"
else
    say "      $(printf '%s' "$started" | head -4)"
    fail "the one command did not find what it needed: ${started}"
fi

if printf '%s' "$started" | grep -q '5 event(s) sent to position 4 stepwise'; then
    say "PASS  and what it started was a real run over the whole recorded market"
else
    fail "no run reached the supervisor: ${started}"
fi

if printf '%s' "$started" | grep -q 'next:     bijective-promote'; then
    say "PASS  and it names the command that answers what happens next"
else
    fail "the first command did not name the second: ${started}"
fi

# 7: Python: the run environment is built from nothing, so where a strategy imports from is declared and is part of the run.
py_starter="${first_run}/first_strategy.py"
if "$BACKTEST" --new "$py_starter" >/dev/null 2>&1 && [ -x "$py_starter" ]; then
    say "PASS  the scaffold writes a Python strategy too, with an interpreter it named absolutely"
else
    say "      $("$BACKTEST" --new "${first_run}/probe.py" 2>&1 | head -2)"
    fail "no Python strategy was written at ${py_starter}"
fi

python_run="$(cd "$first_run" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "$BACKTEST" "$py_starter" 2>&1)"
if printf '%s' "$python_run" | grep -q 'library:.*PYTHONPATH'; then
    say "PASS  and running it declares where the library is, rather than hoping it is installed"
else
    fail "the library was not declared for a Python strategy: ${python_run}"
fi

# The assertion that matters: it imported, it read its window, and what it said was admitted.
if printf '%s' "$python_run" | grep -qE '5 event\(s\) sent to position 4 stepwise.*[1-9][0-9]* proposal\(s\) admitted, 0 refused'; then
    say "PASS  a Python strategy imported its library and its proposals were admitted"
else
    say "      $(printf '%s' "$python_run" | tail -2)"
    fail "the Python strategy did not run: ${python_run}"
fi

# 8: from nothing: no store, no market, no project file, no flags. A sample market ships with the command.
scratch="${WORK}/from-nothing"
mkdir -p "$scratch"
made="$(cd "$scratch" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "$BACKTEST" --new alpha.sh 2>&1)"

if printf '%s' "$made" | grep -q 'seeded market-sample' \
    && printf '%s' "$made" | grep -q 'a sample, not a venue recording'; then
    say "PASS  a first strategy gets a market without a venue, and it is named a sample"
else
    fail "nothing was seeded from an empty directory: ${made}"
fi

if [ -f "${scratch}/bijective.json" ] && grep -q '"market": "market-sample"' "${scratch}/bijective.json"; then
    say "PASS  and what does not change between runs was written down, not left to be retyped"
else
    fail "no project file was written in ${scratch}"
fi

# The assertion that matters: the next command carries no flags and still knows everything.
from_nothing="$(cd "$scratch" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "$BACKTEST" alpha.sh 2>&1)"
if printf '%s' "$from_nothing" | grep -q '500 event(s) sent to position 499 stepwise' \
    && printf '%s' "$from_nothing" | grep -q 'next:     bijective-promote.*19600'; then
    say "PASS  two commands from an empty directory to a verdict, with no flags and no venue"
else
    say "      $(printf '%s' "$from_nothing" | tail -2)"
    fail "the second command did not stand on the project file: ${from_nothing}"
fi

# 9: the declared PYTHONPATH belongs to the run, not the strategy digest, or the promotion stages reports nothing recorded.
py_stages="$("$PROMOTE" "${first_run}/store" "$py_starter" 19600 2>&1)"
if printf '%s' "$py_stages" | grep -q 'backtest:' && ! printf '%s' "$py_stages" | grep -q 'nothing recorded'; then
    say "PASS  the promotion stages sees a Python strategy's runs, not only a shell strategy's"
else
    fail "the promotion stages cannot see a Python strategy: ${py_stages}"
fi

# 10: from an unpacked artefact outside any repository (bin/, lib/python/, no install or root). A delegated subtree stays required.
kit="${WORK}/kit"
mkdir -p "${kit}/bin" "${kit}/lib/python"
cp "$SUPERVISOR" "$PROMOTE" "$BACKTEST" "${kit}/bin/"
cp -r "${CORE}/python/bijective-strategy/src/bijective_strategy" "${kit}/lib/python/"

evaluated="${WORK}/evaluating"
mkdir -p "$evaluated"
kit_made="$(cd "$evaluated" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "${kit}/bin/bijective-backtest" --new tryout.py 2>&1)"
kit_run="$(cd "$evaluated" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "${kit}/bin/bijective-backtest" tryout.py 2>&1)"

if printf '%s' "$kit_run" | grep -q "library:  ${kit}/lib/python"; then
    say "PASS  an unpacked artefact finds its own library, beside the binary rather than in a repository"
else
    say "      $(printf '%s' "$kit_made" | head -2)"
    fail "the kit did not find its library: $(printf '%s' "$kit_run" | head -3)"
fi

if printf '%s' "$kit_run" | grep -q '500 event(s) sent to position 499 stepwise'; then
    say "PASS  and a strategy runs from it with nothing installed at all"
else
    fail "nothing ran from the unpacked artefact: ${kit_run}"
fi

# 11: the scaffold does not overwrite an operator's work.
again="$(cd "$evaluated" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "${kit}/bin/bijective-backtest" --new tryout.py 2>&1)"
if printf '%s' "$again" | grep -q 'is already there' \
    && grep -q '"market": "market-sample"' "${evaluated}/bijective.json"; then
    say "PASS  a second scaffold refuses the strategy that is there and leaves the project alone"
else
    fail "the scaffold overwrote something: ${again}"
fi

# 12: no subtree named: a new scope is made, delegated and named. An existing BIJECTIVE_CGROUP_ROOT is never overridden.
unbounded="${WORK}/unbounded"
mkdir -p "$unbounded"
made_own="$(cd "$unbounded" \
    && env -u BIJECTIVE_CGROUP_ROOT "${kit}/bin/bijective-backtest" --new own.sh >/dev/null 2>&1 \
    && cd "$unbounded" && env -u BIJECTIVE_CGROUP_ROOT "${kit}/bin/bijective-backtest" own.sh 2>&1)"

if printf '%s' "$made_own" | grep -q "bounded:.*a scope of this session's own"; then
    say "PASS  with no subtree named, it makes one of its own and says which"
else
    say "      $(printf '%s' "$made_own" | head -3)"
    fail "nothing was bounded and nothing was said: ${made_own}"
fi

if printf '%s' "$made_own" | grep -q '500 event(s) sent to position 499 stepwise'; then
    say "PASS  and the run inside it is the same bounded run as any other"
else
    fail "the self-bounded run did not happen: ${made_own}"
fi

# 13: the self-made scope must not break: failure inside must fail outside, and the marker must prevent a second scope.
printf '#!/bin/sh\nexit 1\n' > "${unbounded}/crashes.sh"
chmod +x "${unbounded}/crashes.sh"
(cd "$unbounded" && env -u BIJECTIVE_CGROUP_ROOT "${kit}/bin/bijective-backtest" crashes.sh >/dev/null 2>&1)
crashed=$?
if [ "$crashed" != 0 ]; then
    say "PASS  a run that fails inside the scope fails outside it (exit ${crashed})"
else
    fail "a failing run reported success once it crossed the scope; every script reading it is wrong"
fi

# With the marker set the command must delegate here or say why not. A second scope would recurse and report a foreign subtree.
own_scope="/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup)"
reused="$(cd "$unbounded" && BIJECTIVE_SCOPE=1 env -u BIJECTIVE_CGROUP_ROOT \
    "${kit}/bin/bijective-backtest" own.sh 2>&1)"
if printf '%s' "$reused" | grep -q "bounded:  ${own_scope}/" \
    || printf '%s' "$reused" | grep -qE 'delegates no controllers|could not be made'; then
    say "PASS  and with the marker set it never asks for a second scope: it delegates here, or says why not"
else
    say "      $(printf '%s' "$reused" | head -2)"
    fail "a second scope was started despite the marker: ${reused}"
fi

# 14: the page and the command must say the same thing. Two renderings of one stage sequence drift.
REPORT="${CORE}/target/debug/bijective-report"
page="$("$REPORT" "${WORK}/store" "${WORK}/steady.sh" 19600 2>&1)"
spoken="$("$PROMOTE" "${WORK}/store" "${WORK}/steady.sh" 19600 2>&1)"

if printf '%s' "$page" | grep -q '^# steady.sh' \
    && printf '%s' "$page" | grep -q '^## Runs' \
    && printf '%s' "$page" | grep -q '^## Where it stands'; then
    say "PASS  a strategy has a page: what ran, what was claimed, and where it stands"
else
    say "      $(printf '%s' "$page" | head -3)"
    fail "no page was produced for a strategy with runs on the ledger"
fi

# Starts from what have I run, so a researcher need not know the name first.
listing="$("$REPORT" "${WORK}/store" 2>&1)"
if printf '%s' "$listing" | grep -q '^# Strategies in' \
    && printf '%s' "$listing" | grep -q 'steady.sh' \
    && printf '%s' "$listing" | grep -qE '\| 20[0-9][0-9]-[0-9][0-9]-[0-9][0-9] '; then
    say "PASS  a store says what has run against it, by name and by when"
else
    say "      $(printf '%s' "$listing" | head -4)"
    fail "the store cannot say what has been run against it: ${listing}"
fi

# Times render as a person reads them, not as raw _ns.
if printf '%s' "$page" | grep -qE '\| 20[0-9][0-9]-[0-9][0-9]-[0-9][0-9] [0-9][0-9]:[0-9][0-9]:[0-9][0-9]Z \|' \
    && printf '%s' "$page" | grep -q 'Oldest first'; then
    say "PASS  and it says when each run happened, oldest first, so a figure has something to move against"
else
    say "      $(printf '%s' "$page" | sed -n '/## Runs/,+4p' | head -4)"
    fail "the page carries figures with no time against them"
fi

# A strategy keeps its history when its file moves: identity is the declared name, not the path.
moved="${WORK}/moved"
mkdir -p "$moved"
(cd "$moved" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "${kit}/bin/bijective-backtest" --new travelling.sh >/dev/null 2>&1)
(cd "$moved" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "${kit}/bin/bijective-backtest" travelling.sh >/dev/null 2>&1)
mkdir -p "${moved}/elsewhere"
mv "${moved}/travelling.sh" "${moved}/elsewhere/renamed.sh"
(cd "$moved" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "${kit}/bin/bijective-backtest" elsewhere/renamed.sh >/dev/null 2>&1)

kept="$("${kit}/bin/bijective-promote" "${moved}/store" travelling 19600 2>&1)"
if printf '%s' "$kept" | grep -q 'backtest:' && ! printf '%s' "$kept" | grep -q 'nothing recorded'; then
    say "PASS  a strategy keeps its history when its file is moved and renamed"
else
    fail "moving the file lost the promotion stages: ${kept}"
fi

# Which bytes ran: a figure measured on one version says nothing about another.
changed="${WORK}/changed"
mkdir -p "$changed"
(cd "$changed" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "${kit}/bin/bijective-backtest" --new drifting.sh >/dev/null 2>&1)
(cd "$changed" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "${kit}/bin/bijective-backtest" drifting.sh >/dev/null 2>&1)
printf '\n# the same behaviour, different bytes\n' >> "${changed}/drifting.sh"
(cd "$changed" && BIJECTIVE_CGROUP_ROOT="$CGROUP" "${kit}/bin/bijective-backtest" drifting.sh >/dev/null 2>&1)
drifted="$("$REPORT" "${changed}/store" "${changed}/drifting.sh" 19600 2>&1)"

# Reproduction compares emitted output, so only the recorded bytes show a change that alters nothing.
if printf '%s' "$drifted" | grep -q 'The file changed between these runs' \
    && printf '%s' "$drifted" | grep -q 'identical'; then
    say "PASS  the page says the file changed, even where the two runs emitted the same bytes"
else
    say "      $(printf '%s' "$drifted" | sed -n '/## Runs/,+6p' | head -6)"
    fail "a strategy whose file changed under one name was reported as one strategy: ${drifted}"
fi

# The stream matters, not the class: only it says whether output could reach a venue, so a paper figure is not read as live.
if printf '%s' "$page" | grep -q 'proposals landed on' \
    && printf '%s' "$page" | grep -q 'proposals-research' \
    && printf '%s' "$page" | grep -q 'proposals-paper'; then
    say "PASS  and it names where each run's proposals landed, so a paper figure is not a live one"
else
    say "      $(printf '%s' "$page" | sed -n '/## Runs/,+5p' | head -5)"
    fail "the page does not say which stream a run wrote to"
fi

# Figures are read back from the promotion stages's measurement, never recomputed on the page.
if printf '%s' "$page" | grep -q '^## What was measured' \
    && printf '%s' "$page" | grep -qE 'observation\(s\) . dispersion' \
    && printf '%s' "$page" | grep -q '^- clearance:' \
    && printf '%s' "$page" | grep -q '^- basis:'; then
    say "PASS  and it carries the figures: observations, dispersion, the floor and the clearance"
else
    say "      $(printf '%s' "$page" | sed -n '/What was measured/,+4p' | head -4)"
    fail "the page states where a strategy stands and not what was measured"
fi

# Compared in both surfaces: a report that agreed with nothing would pass a test written about it alone.
term="$(printf '%s' "$spoken" | sed -n 's/^  backtest: *//p' | head -1)"
if [ -n "$term" ] && printf '%s' "$page" | grep -qF -- "$term"; then
    say "PASS  and it says what the command says, because both read one stage sequence"
else
    say "      command: ${term:-<nothing>}"
    fail "the page and the command disagree about the same strategy"
fi

if [ "$status" = 0 ]; then
    say "PASS  one command, the record, and the next thing to do"
fi
exit "$status"
