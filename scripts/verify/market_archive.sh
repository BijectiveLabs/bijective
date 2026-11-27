#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# The market recording path driven end to end as processes, on recorded venue traffic:
# journal before archive, a verified archive, a byte-identical rebuild from the journal, a crash
# that loses nothing the journal held, compaction proven equal, journal retention that removes only
# what the archive holds, and a measured memory bound.

set -u
status=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL  $*"; status=1; }

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

# Peak resident memory of one recording run, in KiB. Measured at 2026-11-03 on a debug build.
# The bound is the declaration the drill holds every later build to.
MEMORY_BOUND_KIB="${MEMORY_BOUND_KIB:-65536}"

command -v cargo >/dev/null 2>&1 || { say "FAIL  no cargo on PATH, so the recording path is UNVERIFIED"; exit 1; }
# Two builds: `--example` narrows a build to examples, so the programs are built on their own.
if ! out="$(cd "$CORE" && cargo build --quiet --offline -p bijective-data-engine -p bijective-archive 2>&1 \
        && cargo build --quiet --offline -p bijective-venue --example drill_profile 2>&1)"; then
    say "FAIL  the programs did not build, so the recording path is UNVERIFIED"
    printf '%s\n' "$out" | sed 's/^/        /' | head -5
    exit 1
fi

# shellcheck source=scripts/verify/drill_fixtures.sh
. "${CORE}/scripts/verify/drill_fixtures.sh"
DATA_ENGINE="${CORE}/target/debug/bijective-data-engine"
ARCHIVE="${CORE}/target/debug/bijective-archive"
JOURNAL="${CORE}/target/debug/bijective-journal"
for program in "$DATA_ENGINE" "$ARCHIVE" "$JOURNAL"; do
    [ -x "$program" ] || { say "FAIL  ${program##*/} is missing after a successful build"; exit 1; }
done

VENUE="$(cd "${CORE}/crates/adapters/registry" && cargo run --quiet --offline --example list_venues 2>/dev/null | sed -n '1p')"
FIXTURE="$(find "${CORE}/crates/adapters/${VENUE}/fixtures" -name '*.jsonl' 2>/dev/null | sort | head -1)"
[ -r "$FIXTURE" ] || { say "FAIL  no recorded traffic for the first registered venue"; exit 1; }
SYMBOLS="$(grep -o '\\"s\\":\\"[A-Z0-9]*\\"' "$FIXTURE" | sed 's/.*:\\"//; s/\\"//' | sort -u | paste -sd, -)"
[ -n "$SYMBOLS" ] || { say "FAIL  the recording names no instrument"; exit 1; }
FRAMES="$(grep -c . "$FIXTURE")"
mkdir -p "${WORK}/run"
drill_profile "$CORE" "$VENUE" "$SYMBOLS" "${WORK}/profile.json"

# The flags of a run named NAME. Background runs start the binary itself, so `$!` is its pid.
flags_of() {
    FLAGS=(--store "${WORK}/$1" --archive "${WORK}/$1-archive" --readiness "${WORK}/run/$1.json"
        --venue "$VENUE" --profile "${WORK}/profile.json")
}
engine() {
    local name="$1"; shift
    flags_of "$name"
    "$DATA_ENGINE" "${FLAGS[@]}" "$@"
}
verify() { "$ARCHIVE" verify --root "${WORK}/$1-archive" --venue "$VENUE" --price-scale 2 --quantity-scale 3; }
field() { grep -o "[0-9]* $2" <<<"$1" | head -1 | grep -o '^[0-9]*'; }
digests() { (cd "${WORK}/$1-archive/${VENUE}" && find $(ls -d book facts gaps 2>/dev/null) -type f -name '*.parquet' -exec sha256sum {} + | sort); }

# 1. A recording run, held after ingest so its peak memory can be read while it lives.
flags_of first
"$DATA_ENGINE" "${FLAGS[@]}" --recording "$FIXTURE" --drill-seconds 2 >"${WORK}/first.log" 2>&1 &
pid=$!
peak=0
while kill -0 "$pid" 2>/dev/null; do
    now="$(awk '/VmHWM/ {print $2}' "/proc/${pid}/status" 2>/dev/null || true)"
    [ -n "$now" ] && [ "$now" -gt "$peak" ] && peak="$now"
    sleep 0.1
done
wait "$pid" || fail "the recording run exited $?: $(tail -1 "${WORK}/first.log")"
detail="$(cat "${WORK}/run/first.json" 2>/dev/null)"
journaled="$(field "$detail" 'frame(s) journaled')"
if [ "${journaled:-0}" = "$FRAMES" ]; then
    say "PASS  every one of ${FRAMES} recorded frames was journaled before it was read"
else
    fail "${journaled:-no} frame(s) journaled of ${FRAMES} recorded"
fi
if [ "$peak" -gt 0 ] && [ "$peak" -le "$MEMORY_BOUND_KIB" ]; then
    say "PASS  the recording run peaked at ${peak} KiB, within the declared ${MEMORY_BOUND_KIB} KiB"
else
    fail "the recording run peaked at ${peak} KiB against a bound of ${MEMORY_BOUND_KIB} KiB"
fi

# 2. The archive verifies against its manifest, and the session is marked at both ends.
if report="$(verify first)" && [ "$(field "$report" 'gap row(s)')" = 2 ]; then
    say "PASS  the archive verifies: ${report}"
else
    fail "the archive does not verify as a whole session: ${report}"
fi

# 3. A rebuild from the journal alone reproduces the archive byte for byte.
engine rebuilt --replay-store "${WORK}/first" --drill-seconds 0 >"${WORK}/rebuilt.log" 2>&1 \
    || fail "the rebuild from the journal exited non-zero: $(tail -1 "${WORK}/rebuilt.log")"
if [ -n "$(digests first)" ] && [ "$(digests first)" = "$(digests rebuilt)" ]; then
    say "PASS  a rebuild from the journal is byte-identical to the archive it replaces"
else
    fail "the rebuilt archive differs from the original"
fi

# 4. A crash mid-ingest: the restart refuses the stale writer, and the journal rebuilds the rest.
flags_of crashed
"$DATA_ENGINE" "${FLAGS[@]}" --recording "$FIXTURE" --drill-seconds 30 >"${WORK}/crashed.log" 2>&1 &
pid=$!
sleep 0.5
if kill -9 "$pid" 2>/dev/null; then
    wait "$pid" 2>/dev/null
    if engine crashed --recording "$FIXTURE" --drill-seconds 0 >/dev/null 2>&1; then
        fail "a restart took a writer the killed process still held"
    else
        say "PROVEN a restart refuses the killed run's writer rather than racing it"
    fi
    # Clearing the locks is an operator's act: the platform will not decide a writer is gone.
    find "${WORK}/crashed" "${WORK}/crashed-archive" -name '.writer.lock' -delete
    engine recovered --replay-store "${WORK}/crashed" --drill-seconds 0 >"${WORK}/recovered.log" 2>&1 \
        || fail "the journal of the killed run could not be replayed: $(tail -1 "${WORK}/recovered.log")"
    recovered="$(field "$(cat "${WORK}/run/recovered.json")" 'frame(s) journaled')"
    report="$(verify recovered)"
    if [ "${recovered:-0}" -gt 0 ] && [ "$recovered" -lt "$FRAMES" ] && [ "$(field "$report" 'gap row(s)')" = 1 ]; then
        say "PASS  the killed run's ${recovered} journaled frames rebuild a sound archive whose session has no end"
    else
        fail "the killed run did not rebuild as an open session (${recovered:-no} frames): ${report}"
    fi
    engine crashed --recording "$FIXTURE" --drill-seconds 0 >"${WORK}/restart.log" 2>&1
    if grep -q 'previous session did not close' "${WORK}/restart.log"; then
        say "PASS  reopening the killed run's archive reports that its last session never closed"
    else
        fail "reopening the killed run's archive did not report the unclosed session"
    fi
else
    fail "the run ended before it could be killed, so the crash is UNVERIFIED"
fi

# 5. A stop request ends the program cleanly instead of waiting out its hold.
flags_of stopped
"$DATA_ENGINE" "${FLAGS[@]}" --recording "$FIXTURE" --drill-seconds 60 >"${WORK}/stopped.log" 2>&1 &
pid=$!
waited=0
until grep -q '"health": "ready"' "${WORK}/run/stopped.json" 2>/dev/null || [ "$waited" -ge 300 ]; do
    sleep 0.1; waited=$((waited + 1))
done
kill -TERM "$pid" 2>/dev/null
waited=0
while kill -0 "$pid" 2>/dev/null && [ "$waited" -lt 50 ]; do sleep 0.1; waited=$((waited + 1)); done
if kill -0 "$pid" 2>/dev/null; then
    kill -9 "$pid" 2>/dev/null
    fail "a stop request was not honoured within 5 seconds"
elif wait "$pid" && grep -q 'stop requested' "${WORK}/stopped.log"; then
    say "PASS  a stop request ends the program with exit 0 and its archive closed"
else
    fail "a stop request did not end the program cleanly: $(tail -1 "${WORK}/stopped.log")"
fi

# 6. Two sessions in one partition compact into one file holding exactly their rows.
engine first --recording "$FIXTURE" --drill-seconds 0 >/dev/null 2>&1 || fail "a second session on the first store failed"
before="$(field "$(verify first)" 'book row(s)')"
date="$(cd "${WORK}/first-archive/${VENUE}/book" && ls | head -1 | sed 's/^date=//')"
if "$ARCHIVE" compact --root "${WORK}/first-archive" --venue "$VENUE" --price-scale 2 --quantity-scale 3 \
        --table book --date "$date" >"${WORK}/compact.log" 2>&1 \
        && after="$(verify first)" && [ "$(field "$after" 'book row(s)')" = "$before" ] \
        && [ "$(find "${WORK}/first-archive/${VENUE}/book" -name '*.parquet' | wc -l)" = 1 ]; then
    say "PASS  compaction merged a partition into one file of the same ${before} rows, and it verifies"
else
    fail "compaction did not leave one verified file of ${before} rows: $(cat "${WORK}/compact.log")"
fi

# 7. Retention removes the journal segments the verified archive holds, and only those.
engine kept --recording "$FIXTURE" --drill-seconds 0 --journal-segment-bytes 65536 >/dev/null 2>&1 \
    || fail "the retention run failed"
segments() { find "${WORK}/$1/frames-${VENUE}" -name '*.seg' | wc -l; }
prune() {
    "$JOURNAL" prune --store "${WORK}/$1" --venue "$VENUE" --archive "${WORK}/$2-archive" \
        --price-scale 2 --quantity-scale 3
}
held="$(segments kept)"
if prune kept first >"${WORK}/refused.log" 2>&1 || [ "$(segments kept)" != "$held" ]; then
    fail "a prune against another store's archive was not refused, or removed segments"
else
    say "PASS  a prune against an archive written from another journal is refused and removes nothing"
fi
if out="$(prune kept kept 2>&1)" && left="$(segments kept)" && [ "$left" -lt "$held" ] && [ "$left" -ge 1 ] \
        && verify kept >/dev/null; then
    say "PASS  retention removed $((held - left)) of ${held} journal segment(s); the archive still verifies"
else
    fail "retention did not remove journal segments the archive holds: ${out}"
fi
if again="$(prune kept kept 2>&1)" && grep -q '^pruned 0 segment' <<<"$again" && [ "$(segments kept)" = "$left" ]; then
    say "PASS  a second prune finds nothing more to remove"
else
    fail "a second prune was not a no-op: ${again}"
fi
if engine replayed-kept --replay-store "${WORK}/kept" --drill-seconds 0 >"${WORK}/replay-kept.log" 2>&1; then
    say "PASS  a pruned journal replays from its horizon"
else
    fail "a pruned journal did not replay from its horizon: $(tail -1 "${WORK}/replay-kept.log")"
fi

# A trade frame, the venue's own documented example, appended to the recording: the data engine
# puts it on the facts stream under the platform's identity, where a strategy reads it.
DOCUMENTED_TRADE="${CORE}/crates/adapters/${VENUE}/fixtures/documented/public_trade.json"
if [ -r "$DOCUMENTED_TRADE" ]; then
    python3 - "$FIXTURE" "$DOCUMENTED_TRADE" "${WORK}/with-trade.jsonl" <<'PY'
import json, sys
recording, trade, out = sys.argv[1:4]
lines = [l for l in open(recording).read().splitlines() if l.strip()]
last = int(json.loads(lines[-1])["at_ns"])
lines.append(json.dumps({"at_ns": str(last + 1), "frame": open(trade).read().strip()}))
open(out, "w").write("\n".join(lines) + "\n")
PY
    engine traded --recording "${WORK}/with-trade.jsonl" --drill-seconds 0 >"${WORK}/traded.log" 2>&1 \
        || fail "the recording with a trade did not run: $(tail -1 "${WORK}/traded.log")"
    facts="$(cd "${CORE}/crates/serialization" \
        && cargo run --quiet --offline --example read_facts -- "${WORK}/traded" "$VENUE" 2>/dev/null)"
    if printf '%s' "$facts" | grep -q "^0 trade ${VENUE}\."; then
        say "PASS  a recorded trade reached the facts stream under the platform's identity"
    else
        fail "a recorded trade did not reach the facts stream: ${facts}"
    fi
fi

if [ "$status" -eq 0 ]; then
    say "PASS  the recording path journals first, verifies, rebuilds identically and survives a crash"
fi
exit "$status"
