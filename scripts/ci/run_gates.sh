#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Bijective Labs

# Executes the change gates specified in ci_gates.md. A gate that cannot run here is a FAILURE, never
# a skip: an unverified change is reported as unverified (Coding_Standards section 2).
# Usage:  bash scripts/ci/run_gates.sh
# Exit:   0 every gate passed, 1 at least one gate failed or could not be run.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CORE="${REPO_ROOT}"

# Toolchain is located, not assumed: a gate that fails for the wrong reason teaches people to ignore it.
export CARGO_HOME="${CARGO_HOME:-${HOME}/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-${HOME}/.rustup}"

# Order: explicit RUSTUP_HOME, legacy workstation path, then PATH. The pinned version comes from a manifest.
PINNED_RUST="$(find "${CORE}" -name Cargo.toml -not -path '*/target/*' -exec grep -h '^rust-version' {} + 2>/dev/null | head -1 | sed 's/.*"\(.*\)".*/\1/')"
TOOLCHAIN_ROOT=""
for root in "${RUSTUP_HOME}" /app/.rustup; do
    [ -d "$root/toolchains" ] || continue
    for tc in "$root"/toolchains/${PINNED_RUST}-* "$root"/toolchains/*; do
        [ -d "$tc/bin" ] || continue
        # Several binaries here are rustup proxies (cargo-clippy) that read RUSTUP_HOME, not PATH, so the
        # environment must name the root that supplied the toolchain.
        TOOLCHAIN_ROOT="$root"
        export RUSTUP_HOME="$root"
        export PATH="${tc}/bin:${PATH}"
        break 2
    done
done

# A project venv is preferred for its test dependencies. A bare interpreter is accepted so a fresh
# machine reports accurately instead of not running.
PYTHON=""
PYTHON_FLOOR="$(find "${CORE}" -name pyproject.toml -not -path '*/.venv/*' -exec grep -h 'requires-python' {} + 2>/dev/null \
    | head -1 | grep -oE '3\.[0-9]+' | head -1)"

# The interpreter is checked against the floor the packages declare: the first python3 on PATH
# can sit below it and fail the tests for a reason unrelated to the change.
for candidate in "${CORE}"/python/bijective-attribution/.venv/bin/python "python${PYTHON_FLOOR}" "$(command -v python3 || true)"; do
    [ -n "$candidate" ] || continue
    cand_path="$(command -v "$candidate" 2>/dev/null || printf '%s' "$candidate")"
    [ -x "$cand_path" ] || continue
    if [ -n "$PYTHON_FLOOR" ]; then
        "$cand_path" -c "import sys;raise SystemExit(0 if sys.version_info[:2] >= tuple(int(p) for p in '${PYTHON_FLOOR}'.split('.')) else 1)" 2>/dev/null || continue
    fi
    PYTHON="$cand_path"
    break
done

# The registry is chosen by resolving a real manifest offline, not by counting crates: a large
# registry can still miss the one version a lock pins. The probe is the deepest tree, discovered
# rather than named, so the harness is tied to no component.
probe_manifest=""
probe_depth=0
while IFS= read -r lock; do
    depth="$(grep -c '^\[\[package\]\]' "$lock" 2>/dev/null || true)"
    if [ "${depth:-0}" -gt "$probe_depth" ]; then
        probe_depth="$depth"
        probe_manifest="$(dirname "$lock")/Cargo.toml"
    fi
done < <(find "${CORE}" -name Cargo.lock -not -path '*/target/*' 2>/dev/null | sort)

if [ -n "$probe_manifest" ] && command -v cargo >/dev/null 2>&1; then
    registry_tried=""
    for home in "${CARGO_HOME}" "${TOOLCHAIN_ROOT%.rustup}.cargo" "${HOME}/.cargo" /app/.cargo; do
        [ -d "${home}/registry" ] || continue
        case " ${registry_tried} " in *" ${home} "*) continue ;; esac
        registry_tried="${registry_tried} ${home}"
        if CARGO_HOME="$home" cargo metadata --offline --format-version 1 \
               --manifest-path "$probe_manifest" >/dev/null 2>&1; then
            export CARGO_HOME="$home"
            registry_tried=""
            break
        fi
    done
    if [ -n "$registry_tried" ]; then
        echo "note: no package registry resolves this workspace offline (tried:${registry_tried})"
    fi
fi


# Every component belongs to exactly one architectural area, declared in areas.txt beside this
# runner. The table is the single source for the structure and dependency-direction gates.
AREAS_FILE="${CORE}/scripts/ci/areas.txt"

area_dirs() {
    awk -v want="$1" '$0 !~ /^#/ && NF == 2 && $2 == want { print ENVIRON["CORE"] "/" $1 }' "$AREAS_FILE"
}
export CORE

# Subjects are discovered, not listed: a gate that enumerates its own subjects silently skips
# whatever is added next. Paths are relative to CORE so two crates of one name cannot collide.
# A workspace root (`[workspace]`, no `[package]`) builds nothing and is not a component.
is_component_manifest() {
    grep -q '^\[package\]' "$1" 2>/dev/null
}

rust_crates() {
    local m d
    while IFS= read -r m; do
        is_component_manifest "$m" || continue
        d="$(dirname "$m")"
        printf '%s\n' "${d#"${CORE}/"}"
    done < <(find "${CORE}" -name Cargo.toml -not -path '*/target/*' | sort)
}

python_packages() {
    local m d
    while IFS= read -r m; do
        d="$(dirname "$m")"
        printf '%s\n' "${d#"${CORE}/"}"
    done < <(find "${CORE}" -name pyproject.toml -not -path '*/.venv/*' | sort)
}

failures=0
declare -a results=()

record() {
    # record <PASS|FAIL|NOT RUN> <gate name> <detail> <subjects examined>
    # A pass over nothing is a failure: a find or grep that matched no files produces no hits, and no
    # hits reads as clean. The subject count is enforced here rather than trusted to each gate.
    local status="$1" gate="$2" detail="$3" examined="${4-}"
    if [ "$status" = "PASS" ]; then
        case "$examined" in
            '')       status="FAIL"; detail="passed without stating what it examined" ;;
            *[!0-9]*) status="FAIL"; detail="subject count is not a number: ${examined}" ;;
            0)        status="FAIL"; detail="examined nothing; ${detail}" ;;
        esac
    fi
    results+=("${status}|${gate}|${examined:--}|${detail}")
    [ "$status" = "PASS" ] || failures=$((failures + 1))
}

# Searches our own work only: a vendored dependency's use of a restricted word is not our
# violation, and a scan that reported it would be muted.
scan_company() {
    grep -rIn "$@" "${REPO_ROOT}" 2>/dev/null \
        | grep -v '/\.git/' \
        | grep -v '/\.venv/' \
        | grep -v '/__pycache__/' \
        | grep -v '/target/' \
        || true
}

# Coverage is measured here so a scan and the number it reports cannot drift apart.
count_files() { find "$@" -type f 2>/dev/null | grep -c . || true; }

RUST_SOURCES="$(count_files "${CORE}" -name '*.rs' -not -path '*/target/*')"
SOURCE_FILES="$(count_files "${CORE}" \( -name '*.rs' -o -name '*.py' \) \
    -not -path '*/target/*' -not -path '*/.venv/*' -not -path '*/__pycache__/*')"
CORE_FILES="$(count_files "${CORE}" -not -path '*/target/*' -not -path '*/.venv/*' \
    -not -path '*/__pycache__/*')"

# Steps a drill asserted: an exit status cannot tell a drill that checked five things from none.
drill_steps() { printf '%s\n' "$1" | grep -c '^PASS' || true; }


gate_venue_names() {
    # The adapter contract: no component above the adapter contract may name a venue.
    # Deliberately a crude grep: it keeps working after everyone who agreed to the rule has left.
    local list="${CORE}/scripts/ci/venue_names.txt"
    if [ ! -r "$list" ]; then
        record "NOT RUN" "Venue names" "name list absent at ${list##*/}"
        return
    fi

    local -a patterns=()
    local name
    while IFS= read -r name; do
        [ -z "$name" ] && continue
        case "$name" in \#*) continue ;; esac
        patterns+=(-e "$name")
    done < "$list"

    if [ "${#patterns[@]}" -eq 0 ]; then
        record "NOT RUN" "Venue names" "name list is empty; nothing was checked"
        return
    fi

    # The workspace manifest and Cargo.lock are outside the scan: the manifest's member list is a build list,
    # not a wiring path, and the lock records the resolved transitive graph, so an engine reaching a venue
    # through the registry still shows the adapter there. Cargo.toml and source stay in scope.
    local hits
    hits="$(grep -rIniw "${patterns[@]}" "${CORE}/crates" "${CORE}/python" "${CORE}/scripts/verify" "${CORE}/scripts/measure" 2>/dev/null \
        | grep -v '/target/' | grep -v '/\.venv/' | grep -v '/__pycache__/' \
        | grep -v '/Cargo\.lock:' \
        | grep -v "^${CORE}/Cargo\.toml:" \
        | grep -v "^${CORE}/crates/adapters/" \
        | grep -v "^${CORE}/scripts/ci/venue_names.txt:" || true)"

    if [ -n "$hits" ]; then
        record "FAIL" "Venue names" "$(printf '%s\n' "$hits" | wc -l) mention(s) above the contract"
        printf '%s\n' "$hits" | head -5 | sed "s|^${CORE}/|        |" | cut -c1-110
    else
        record "PASS" "Venue names" "no venue named outside adapters, against $(( ${#patterns[@]} / 2 )) names" "$CORE_FILES"
    fi
}

gate_order_attribution() {
    # No order or request an adapter sends carries a broker id, referral, channel or builder fee
    # unless it is declared and disclosed (docs/concepts/commercial_attribution.md). Two checks:
    # every request header name is on a short allow-list, and none of the attribution words occurs
    # in adapter source. A crude grep on purpose: it keeps working after the people who agreed to
    # the rule have left.
    local headers="${CORE}/scripts/ci/request_headers.txt"
    local declared="${CORE}/scripts/ci/attribution_declared.txt"
    if [ ! -r "$headers" ] || [ ! -r "$declared" ]; then
        record "NOT RUN" "Order attribution" "the header allow-list or the declaration list is absent"
        return
    fi

    local sources
    sources="$(find "${CORE}/crates/adapters" -path '*/src/*' -name '*.rs' -not -path '*/target/*' 2>/dev/null)"
    if [ -z "$sources" ]; then
        record "NOT RUN" "Order attribution" "no adapter source was found"
        return
    fi

    local offenders="" file name
    while IFS= read -r file; do
        while IFS= read -r name; do
            [ -z "$name" ] && continue
            if ! grep -qixF -- "$name" <(grep -v '^#' "$headers"); then
                offenders="${offenders}${file}: sends header ${name}, which is not on the allow-list"$'\n'
            fi
        done < <(grep -oE '\.header\("[^"]+"' "$file" | sed -E 's/^\.header\("//; s/"$//')
    done <<< "$sources"

    local words='broker_?id|x-referer|"referer"|referral|affiliate|x-channel|x-source-key|channel_?code|partner_?id|"builder"|builder_?fee'
    # A declaration names the adapter source file that sends the attribution, first on its line.
    local hits declared_paths
    declared_paths="$(grep -v '^#' "$declared" | grep -v '^$' | awk '{print $1}' || true)"
    hits="$(grep -nIiE -- "$words" $sources 2>/dev/null || true)"
    if [ -n "$declared_paths" ]; then
        hits="$(printf '%s\n' "$hits" | grep -vF -f <(printf '%s\n' "$declared_paths") || true)"
    fi
    # A required field always written empty: accepted only on a line of a listed file that writes
    # the zero value.
    local zeroed="${CORE}/scripts/ci/attribution_zeroed.txt" zeroed_paths
    if [ -r "$zeroed" ]; then
        zeroed_paths="$(grep -v '^#' "$zeroed" | grep -v '^$' | awk '{print $1}' || true)"
        if [ -n "$zeroed_paths" ] && [ -n "$hits" ]; then
            hits="$(printf '%s\n' "$hits" | awk -v list="$zeroed_paths" '
                BEGIN { n = split(list, paths, "\n") }
                { keep = 1; for (i = 1; i <= n; i++) if (index($0, paths[i]) && index($0, "ZERO_WORD")) keep = 0 }
                keep' || true)"
        fi
    fi
    if [ -n "$hits" ]; then
        offenders="${offenders}${hits}"$'\n'
    fi

    if [ -n "$offenders" ]; then
        record "FAIL" "Order attribution" "$(printf '%s' "$offenders" | grep -c .) undeclared attribution(s) in adapter requests"
        printf '%s' "$offenders" | head -20 | sed 's/^/        /'
    else
        record "PASS" "Order attribution" "no broker id, referral or builder fee in any adapter request" "$(printf '%s\n' "$sources" | grep -c .)"
    fi
}

gate_no_float() {
    # Money, price and exposure are never binary floating point.
    # Comment lines and test files are exempt: they state or demonstrate the rule.
    local hits files scanned
    files="$(find "${CORE}" -name '*.rs' -not -path '*/target/*' -not -name 'tests.rs' \
                -not -path '*/tests/*' 2>/dev/null)"
    scanned="$(printf '%s\n' "$files" | grep -c . || true)"
    hits="$(printf '%s\n' "$files" | grep -v '^$' | tr '\n' '\0' \
        | xargs -0 -r grep -nE '\bf32\b|\bf64\b' 2>/dev/null \
        | grep -vE ':[0-9]+:[[:space:]]*(///|//!|//)' || true)"

    if [ -n "$hits" ]; then
        record "FAIL" "No floating point" "$(printf '%s\n' "$hits" | wc -l) float(s) outside tests and comments"
        printf '%s\n' "$hits" | head -5 | sed 's|^'"${CORE}"'/|        |'
    else
        record "PASS" "No floating point" "no float in any non-test source" "$scanned"
    fi
}


gate_structure() {
    # Every component is declared in areas.txt, and every declared component exists.
    local manifest rel bad="" examined=0
    while IFS= read -r manifest; do
        case "$manifest" in
            *Cargo.toml) is_component_manifest "$manifest" || continue ;;
        esac
        examined=$(( examined + 1 ))
        rel="$(dirname "$manifest")"
        rel="${rel#"${CORE}/"}"
        awk -v want="$rel" '$0 !~ /^#/ && $1 == want { found = 1 } END { exit !found }' "$AREAS_FILE" \
            || bad+="
        ${rel} is not declared in areas.txt"
    done < <(find "${CORE}" \( -name Cargo.toml -o -name pyproject.toml \) \
                 -not -path '*/target/*' -not -path '*/.venv/*' | sort)

    local declared
    while IFS= read -r declared; do
        [ -d "${CORE}/${declared}" ] || bad+="
        ${declared} is declared in areas.txt and does not exist"
    done < <(awk '$0 !~ /^#/ && NF == 2 { print $1 }' "$AREAS_FILE")

    if [ -n "$bad" ]; then
        record "FAIL" "Structure" "$(printf '%s' "$bad" | grep -c .) component(s) out of step with areas.txt"
        printf '%s\n' "$bad"
    else
        record "PASS" "Structure" "every component declared in one architectural area" "$examined"
    fi
}

gate_dependency_licences() {
    # OSS Adoption Policy P8: every dependency licence passes an allow-list check in CI.
    # Attested from each package's own manifest. Unattested or other-version packages fail.
    # Test on a copied tree: gate_tests runs cargo first, which rewrites a hand-edited lock.
    local attest="${CORE}/scripts/ci/licence_attestation.tsv"
    if [ ! -r "$attest" ]; then
        record "NOT RUN" "Dependency licences" "attestation absent at ${attest##*/}"
        return
    fi
    if [ ! -x "$PYTHON" ]; then
        record "NOT RUN" "Dependency licences" "no interpreter to read the lock files with"
        return
    fi

    local out
    out="$("$PYTHON" - "$attest" "$CORE" <<'PYEOF'
import pathlib, re, sys

attest_path, core = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
ALLOW = {"MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause",
         "BSD-3-Clause", "ISC", "MPL-2.0", "BSL-1.0", "Zlib", "0BSD",
         "Unicode-3.0", "Unicode-DFS-2016", "CC0-1.0", "MIT-0", "Unlicense",
         # Admissible: section 3.1 (no restrictions on results) reaches the data, not what is built with it.
         # Its section 2.1 notice obligation is discharged by the Third-party notices gate.
         "CDLA-Permissive-2.0"}

attested = {}
for line in attest_path.read_text().splitlines():
    if line.startswith("#") or not line.strip():
        continue
    name, version, licence = line.split("\t")
    attested[(name, version)] = licence

def satisfies(expr):
    tokens = {t.strip() for t in re.split(r"\bOR\b|\bAND\b|/|\(|\)", expr)} - {""}
    if " OR " in expr or "/" in expr:
        return bool(tokens & ALLOW)
    return tokens <= ALLOW

problems, checked, locks = [], 0, 0
for lock in core.rglob("Cargo.lock"):
    if "target" in lock.parts:
        continue
    locks += 1
    text = lock.read_text()
    for name, version in re.findall(r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"', text):
        if name.startswith("bijective-"):
            continue
        checked += 1
        licence = attested.get((name, version))
        if licence is None:
            problems.append(f"{name} {version}: not attested")
        elif not satisfies(licence):
            problems.append(f"{name} {version}: {licence} is not on the allow-list")

print(f"CHECKED\t{checked}\t{len(attested)}\t{locks}")
for p in sorted(set(problems)):
    print(f"PROBLEM\t{p}")
PYEOF
)"

    local summary problems
    summary="$(printf '%s\n' "$out" | awk -F'\t' '$1=="CHECKED"{print $2" "$3" "$4}')"
    problems="$(printf '%s\n' "$out" | awk -F'\t' '$1=="PROBLEM"{print $2}')"
    set -- $summary
    local checked="${1:-0}" attested_n="${2:-0}" locks="${3:-0}"

    if [ -n "$problems" ]; then
        record "FAIL" "Dependency licences" "$(printf '%s\n' "$problems" | wc -l) package(s) unattested or off the allow-list"
        printf '%s\n' "$problems" | head -5 | sed 's/^/        /'
    elif [ "$checked" -eq 0 ]; then
        record "PASS" "Dependency licences" "no third-party package in any lock; ${attested_n} attested and ready" "$locks"
    else
        record "PASS" "Dependency licences" "${checked} third-party package(s), every one attested and allow-listed" "$checked"
    fi
}

gate_agent_environment() {
    # The platform architecture: the agent environment holds no credential, opens no venue connection and has
    # no route to the order path except emitting a signal. Checked on the dependency graph, where it
    # happens through one line in a manifest. The no-network test is required by name: it would rot first.
    local -a engine=()
    mapfile -t engine < <(area_dirs strategy)
    if [ "${#engine[@]}" -eq 0 ]; then
        record "NOT RUN" "Agent environment" "no strategy engine in the tree"
        return
    fi

    local forbidden='bijective-secrets-broker|bijective-venue|bijective-limit|bijective-risk|bijective-control'
    local offenders=""
    local manifest deps name
    while IFS= read -r manifest; do
        deps="$(sed -n '/^\[dependencies\]/,/^\[/p' "$manifest" \
            | grep -vE '^\[|^[[:space:]]*$|^#' || true)"
        if printf '%s\n' "$deps" | grep -E "^(${forbidden})" >/dev/null; then
            name="$(dirname "$manifest")"
            offenders="${offenders} ${name#"${CORE}/"}"
        fi
    done < <(find "${engine[@]}" -name Cargo.toml -not -path '*/target/*' | sort)

    if [ -n "$offenders" ]; then
        record "FAIL" "Agent environment" "credential or venue reach in:${offenders}"
        return
    fi

    local probe="${CORE}/crates/sandbox/tests/unit/tests.rs"
    if [ ! -r "$probe" ]; then
        record "FAIL" "Agent environment" "the sandbox has no gates"
        return
    fi
    if ! grep -q 'fn a_run_that_tries_to_reach_the_network_is_refused' "$probe"; then
        record "FAIL" "Agent environment" "no test asserts a run cannot reach a network"
        return
    fi
    if ! grep -q 'fn the_same_run_outside_the_sandbox_reaches_it' "$probe"; then
        # Without it, the refusal above passes whenever the child fails for any reason at all.
        record "FAIL" "Agent environment" "the no-network test has no vacuity check"
        return
    fi

    local crates
    crates="$(find "${engine[@]}" -name Cargo.toml -not -path '*/target/*' | wc -l | tr -d ' ')"
    record "PASS" "Agent environment" "${crates} crate(s), none reaching a credential or a venue" "$crates"
}

gate_venue_isolation() {
    # Venue isolation driven as two processes: stream writer locks and readiness temp files must
    # not collide between Data Engines sharing a run directory.
    if out="$(bash "${CORE}/scripts/verify/venue_isolation.sh" 2>&1)"; then
        record "PASS" "Venue isolation" "$(echo "$out" | tail -1 | sed 's/^PASS  //')" "$(drill_steps "$out")"
    else
        record "FAIL" "Venue isolation" "$(echo "$out" | grep '^FAIL' | head -1 | sed 's/^FAIL  //')"
        echo "$out" | sed 's/^/        /' | head -8
    fi
}

gate_console_tls() {
    # The console beyond loopback: HTTPS with client certificates, and only its own forms signed.
    if out="$(bash "${CORE}/scripts/verify/console_tls.sh" 2>&1)"; then
        record "PASS" "Console TLS" "$(echo "$out" | tail -1 | sed "s/^PASS  //")" "$(drill_steps "$out")"
    else
        record "FAIL" "Console TLS" "$(echo "$out" | grep "^FAIL" | head -1 | sed "s/^FAIL  //")"
        echo "$out" | sed "s/^/        /" | head -8
    fi
}

gate_market_archive() {
    # The recording path end to end: frames journaled before they are read, an archive that verifies,
    # a rebuild from the journal that is byte-identical, a crash the journal recovers, compaction proven
    # equal, and peak memory within its declared bound.
    if out="$(bash "${CORE}/scripts/verify/market_archive.sh" 2>&1)"; then
        record "PASS" "Market archive" "$(echo "$out" | tail -1 | sed 's/^PASS  //')" "$(drill_steps "$out")"
    else
        record "FAIL" "Market archive" "$(echo "$out" | grep '^FAIL' | head -1 | sed 's/^FAIL  //')"
        echo "$out" | sed 's/^/        /' | head -10
    fi
}

gate_unread_declarations() {
    # A public function nothing calls is a capability declared and not used. Tests are excluded: a
    # function exercised only by its own tests is the shape this gate surfaces. Forward declarations
    # carry a reason. An entry whose function has since gained a reader is stale and fails.
    local allow="${CORE}/scripts/ci/unread_declarations.txt"
    if [ ! -r "$allow" ]; then
        record "NOT RUN" "Unread declarations" "no allowlist alongside this runner"
        return
    fi

    local report
    report="$(CORE="$CORE" ALLOW="$allow" python3 - <<'PYEOF'
import os, re

core, allow = os.environ["CORE"], os.environ["ALLOW"]

def is_test(path):
    b = os.path.basename(path)
    return b == "tests.rs" or b.endswith("_tests.rs") or "/tests/" in path or "/benches/" in path

files = []
for dp, _, fn in os.walk(core):
    if "target" in dp.split(os.sep):
        continue
    files += [os.path.join(dp, f) for f in fn if f.endswith(".rs")]

def without_test_items(text):
    """Remove each #[cfg(test)] item, not everything after the first one.

    Truncating at the first marker dropped 1,517 lines across eight files from every sweep this
    quarter - including most of the adapter contract, whose `#[cfg(test)]` sits two thirds of the
    way up. A file is not test code because it contains a test.
    """
    out, i, n = [], 0, len(text)
    marker = "#[cfg(test)]"
    while True:
        j = text.find(marker, i)
        if j < 0:
            out.append(text[i:])
            return "".join(out)
        out.append(text[i:j])
        k = j + len(marker)
        # Whichever comes first ends the item: a brace block, or a semicolon (`mod tests;`).
        brace, semi = text.find("{", k), text.find(";", k)
        if semi >= 0 and (brace < 0 or semi < brace):
            i = semi + 1
            continue
        if brace < 0:
            return "".join(out)
        depth, m = 1, brace + 1
        while m < n and depth:
            if text[m] == "{":
                depth += 1
            elif text[m] == "}":
                depth -= 1
            m += 1
        i = m

prod = {}
for p in files:
    t = open(p, encoding="utf-8", errors="replace").read()
    prod[p] = "" if is_test(p) else without_test_items(t)

def code_only(text):
    """Drop comments, string and char literals, keeping offsets irrelevant.

    A name written in prose or inside a message reads as a call to an identifier count, and
    between them they hid IdentifierRules::check - whose own doc says "called by whatever mints
    one" and which nothing called. A regex cannot do this: a '"' char literal leaves an unbalanced
    quote and the match then eats real code.
    """
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if c == '/' and i + 1 < n and text[i + 1] == '/':
            while i < n and text[i] != '\n':
                i += 1
        elif c == '/' and i + 1 < n and text[i + 1] == '*':
            depth, i = 1, i + 2
            while i < n and depth:
                if text.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif text.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
        elif c == 'r' and i + 1 < n and text[i + 1] in '"#':
            j = i + 1
            hashes = 0
            while j < n and text[j] == '#':
                hashes, j = hashes + 1, j + 1
            if j < n and text[j] == '"':
                close = '"' + '#' * hashes
                end = text.find(close, j + 1)
                i = n if end < 0 else end + len(close)
            else:
                out.append(c)
                i += 1
        elif c == '"':
            i += 1
            while i < n:
                if text[i] == '\\':
                    i += 2
                elif text[i] == '"':
                    i += 1
                    break
                else:
                    i += 1
        elif c == "'":
            # A lifetime is not a literal: 'a, 'static. A char literal closes within four bytes.
            j = i + 1
            if j < n and text[j] == '\\':
                j += 2
            elif j < n:
                j += 1
            if j < n and text[j] == "'":
                i = j + 1
            else:
                out.append(c)
                i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out)

prod = {p: code_only(v) for p, v in prod.items()}


# Which crates can see a declaration: references are counted in the declaring crate and its
# dependents only, so unrelated items sharing a bare name do not mask each other.
crate_dir = {}
for dp, _, fn in os.walk(core):
    if "target" in dp.split(os.sep):
        continue
    if "Cargo.toml" in fn:
        text = open(os.path.join(dp, "Cargo.toml"), encoding="utf-8", errors="replace").read()
        m = re.search(r'^name\s*=\s*"([^"]+)"', text, re.M)
        if m:
            crate_dir[os.path.abspath(dp)] = m.group(1)

def owning_crate(path):
    p, best = os.path.abspath(path), ""
    for d in crate_dir:
        if p.startswith(d + os.sep) and len(d) > len(best):
            best = d
    return crate_dir.get(best, "")

deps = {}
for d, name in crate_dir.items():
    text = open(os.path.join(d, "Cargo.toml"), encoding="utf-8", errors="replace").read()
    found = set()
    for m in re.finditer(r'path\s*=\s*"([^"]+)"', text):
        target = os.path.abspath(os.path.join(d, m.group(1)))
        if target in crate_dir:
            found.add(crate_dir[target])
    deps[name] = found

visible = {}
for name in set(crate_dir.values()):
    seen, frontier = {name}, [name]
    while frontier:
        cur = frontier.pop()
        for other, ds in deps.items():
            if cur in ds and other not in seen:
                seen.add(other)
                frontier.append(other)
    visible[name] = seen

file_crate = {p: owning_crate(p) for p in files}

# Crates a file may name to reach an item: a type re-exported through an intermediate crate is named
# by the intermediate, never the owner.
reaches = {}
for name_ in set(crate_dir.values()):
    seen, frontier = set(), [name_]
    while frontier:
        cur = frontier.pop()
        for d in deps.get(cur, ()):
            if d not in seen:
                seen.add(d)
                frontier.append(d)
    reaches[name_] = seen

def reaching(from_crate, owner):
    """Crate tokens a file in `from_crate` could plausibly write to reach `owner`."""
    out = {owner}
    for d in deps.get(from_crate, ()):
        if d == owner or owner in reaches.get(d, ()):
            out.add(d)
    return {t.replace("-", "_") for t in out if t}

def count_refs(name, home, constructions_only=False):
    """Occurrences of `name` in production code that could legitimately be using it.

    A reference from another crate only counts if that file names the declaring crate. To reach
    `LimitLedger::flush` from elsewhere a file must import from `bijective_limits`; without that
    rule a `stream.flush()` in an unrelated dependent stood in for the caller, which is exactly
    what happened the first time the dashboard was built.
    """
    if not os.path.isabs(home):
        home = os.path.join(core, home)
    owner = file_crate.get(home, "")
    scope = visible.get(owner)
    pat = re.compile(r'\b' + re.escape(name) + r'\b')

    total = 0
    for p in files:
        crate = file_crate.get(p, "")
        if scope is not None and crate not in scope:
            continue
        if crate != owner and not any(t in prod[p] for t in reaching(crate, owner)):
            continue
        text = built[p] if constructions_only else prod[p]
        # A declaration elsewhere is not a reading of ours: same-named functions in any other file, same
        # crate included, must not count as callers. Only the declaration's own home is excluded.
        if p != home:
            text = re.sub(r'\bpub (?:async )?fn ' + re.escape(name) + r'\b', " ", text)
        total += len(pat.findall(text))
    return total


# A match arm is not a construction: a `Variant { .. } =>` arm would clear a variant nothing builds.
# Patterns are stripped before variants are counted. Functions and constants keep every reference.
_arm = re.compile(r'[A-Za-z_][\w:]*\s*(?:\{[^{}]*\}|\([^()]*\))?\s*(?:\|[^=\n]*)?=>')
built = {p: _arm.sub(" ", prod[p]) for p in files}

decl = re.compile(r'^\s*pub (?:async )?fn ([a-z_][a-z0-9_]*)', re.M)
declared = {}
for p in files:
    for m in decl.finditer(prod[p]):
        declared.setdefault((m.group(1), os.path.relpath(p, core)), p)

# Names the language or a derive calls for us, and entry points.
exempt = re.compile(r'^(new|main|default|fmt|from|from_str|into|next|drop|clone|len|is_empty|run)$')

unread = {}
for (name, rel), home in declared.items():
    if exempt.match(name):
        continue
    if count_refs(name, home) <= 1:
        unread[(name, rel)] = rel

listed = {}
for line in open(allow, encoding="utf-8"):
    line = line.rstrip("\n")
    if not line.strip() or line.lstrip().startswith("#"):
        continue
    parts = line.split("\t")
    if len(parts) < 3 or not parts[2].strip():
        print("MALFORMED\t" + line[:60])
        continue
    # Both columns are read: a name-only key let one allowed declaration cover another crate's.
    key = (parts[0].strip(), parts[1].strip())
    # A duplicate is reported, not overwritten: two reasons for one thing means one is unmaintained.
    if key in listed:
        print("DUPLICATE\t%s\t%s\tlisted more than once" % key)
    listed[key] = parts[2].strip()

for n in sorted(set(unread) - set(listed)):
    print("UNLISTED\t%s\t%s" % (n[0], n[1]))
for n in sorted(set(k for k in listed if "::" not in k[0] and "." not in k[0] and not k[0][:1].isupper()) - set(unread)):
    print("STALE\t%s\t%s\tnow read, or gone" % (n[0], n[1]))

# Appended to gate 31's inner script: enum variants nothing in production can produce.
# Enums deriving Deserialize are skipped: serde builds them from configuration, so a grep would flag all.
def enum_block(text, start):
    i = text.find("{", start)
    if i < 0:
        return ""
    depth, j = 1, i + 1
    while j < len(text) and depth:
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
        j += 1
    return text[i + 1:j - 1]

variants = {}
for p in files:
    body = prod[p]
    for m in re.finditer(r'pub enum (\w+)', body):
        head = body[max(0, m.start() - 400):m.start()]
        if "Deserialize" in head:
            continue
        inner = re.sub(r'\{[^{}]*\}', '', enum_block(body, m.end()))
        for v in re.finditer(r'^\s*([A-Z]\w*)\s*(?:\{|\(|,|=|$)', inner, re.M):
            variants[("%s::%s" % (m.group(1), v.group(1)), os.path.relpath(p, core))] = (v.group(1), os.path.relpath(p, core))

unread_v = {}
for key, (short, home) in variants.items():
    if count_refs(short, home, constructions_only=True) <= 1:
        unread_v[key] = home

for n in sorted(set(unread_v) - set(listed)):
    print("UNLISTED\t%s\t%s" % (n[0], n[1]))
for n in sorted(k for k in listed if "::" in k[0] and "." not in k[0] and k not in unread_v):
    print("STALE\t%s\t%s\tnow produced, or gone" % (n[0], n[1]))

# Struct fields written and never read. An identifier count cannot see them: a field name appears at
# every construction site. A read is a dot access or a destructuring. Everything else is a write.
def struct_body(text, start):
    i = text.find("{", start)
    if i < 0 or ";" in text[start:i]:
        return ""
    depth, j = 1, i + 1
    while j < len(text) and depth:
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
        j += 1
    return text[i + 1:j - 1]

field_home = {}
for p in files:
    b = prod[p]
    for m in re.finditer(r'pub struct (\w+)', b):
        inner = struct_body(b, m.end())
        if not inner:
            continue
        flat = re.sub(r'\{[^{}]*\}', '', inner)
        for f in re.finditer(r'^\s*(?:pub(?:\([^)]*\))?\s+)?(\w+)\s*:', flat, re.M):
            field_home[("%s.%s" % (m.group(1), f.group(1)), os.path.relpath(p, core))] = (f.group(1), os.path.relpath(p, core))

unread_f = {}
for key, (short, home) in field_home.items():
    scope = visible.get(file_crate.get(os.path.join(core, home), ""))
    # A destructuring names its type. A dot access does not. A pattern credits a field only when this
    # file names the owner, while `.field` counts anywhere in scope (inferred types).
    owner = key[0].split(".", 1)[0]
    dot = re.compile(r'\.' + re.escape(short) + r'\b')
    # A destructuring reads a field and a struct literal writes one, and they look alike. A
    # single-line pattern is told apart by what follows its brace (`=>`, `=` or `|`, optionally
    # after closing parentheses). Multi-line reads are matched as the whole `let PATTERN { ... } =`.
    destr = re.compile(
        r'\{[^{}\n]*\b' + re.escape(short) + r'\s*(?:,[^{}\n]*)?\}\)*\s*(?:=>|=(?![=>])|\|)'
    )
    pattern_let = re.compile(
        r'let\s+[A-Z][\w:]*\s*\{[^{}]*\b' + re.escape(short) + r'\s*[,}][^{}]*\}\s*=',
        re.S,
    )
    hits = sum(
        len(dot.findall(prod[p]))
        + (
            len(destr.findall(prod[p])) + len(pattern_let.findall(prod[p]))
            if re.search(r'\b' + re.escape(owner) + r'\b', prod[p])
            else 0
        )
        for p in files
        if scope is None or file_crate.get(p, "") in scope
    )
    if hits == 0:
        unread_f[key] = home

for n in sorted(set(unread_f) - set(listed)):
    print("UNLISTED\t%s\t%s" % (n[0], n[1]))
for n in sorted(k for k in listed if "." in k[0] and "::" not in k[0] and k not in unread_f):
    print("STALE\t%s\t%s\tnow read, or gone" % (n[0], n[1]))

# The sixth path: a field inside an enum variant. The struct sweep anchors a field at line start,
# which is wrong for a variant whose fields sit on one line.
def variant_bodies(body):
    out, i, last = [], 0, None
    while i < len(body):
        ch = body[i]
        if ch.isalpha() or ch == "_":
            j = i
            while j < len(body) and (body[j].isalnum() or body[j] == "_"):
                j += 1
            last, i = body[i:j], j
            continue
        if ch in "{(":
            close = "}" if ch == "{" else ")"
            depth, j = 1, i + 1
            while j < len(body) and depth:
                if body[j] == ch:
                    depth += 1
                elif body[j] == close:
                    depth -= 1
                j += 1
            # A tuple variant has no named fields, so only a braced one is collected.
            if ch == "{" and last:
                out.append((last, body[i + 1:j - 1]))
            i, last = j, None
            continue
        i += 1
    return out

variant_field_home = {}
for p in files:
    b = prod[p]
    for m in re.finditer(r'pub enum (\w+)', b):
        inner = struct_body(b, m.end())
        if not inner:
            continue
        for variant, fields in variant_bodies(inner):
            text = re.sub(r'//[^\n]*', '', fields)
            # `:(?!:)` so a path segment inside a type is not read as a field name.
            for f in re.finditer(r'(?:^|,)\s*(?:pub\s+)?(\w+)\s*:(?!:)', text, re.M):
                key = "%s::%s.%s" % (m.group(1), variant, f.group(1))
                variant_field_home[(key, os.path.relpath(p, core))] = (f.group(1), os.path.relpath(p, core))

unread_vf = {}
for key, (short, home) in variant_field_home.items():
    scope = visible.get(file_crate.get(os.path.join(core, home), ""))
    # The same rule, anchored on the variant: that is what a pattern names.
    owner = key[0].split(".", 1)[0].split("::")[-1]
    dot = re.compile(r'\.' + re.escape(short) + r'\b')
    destr = re.compile(
        r'\{[^{}\n]*\b' + re.escape(short) + r'\s*(?:,[^{}\n]*)?\}\)*\s*(?:=>|=(?![=>])|\|)'
    )
    pattern_let = re.compile(
        r'let\s+[A-Z][\w:]*\s*\{[^{}]*\b' + re.escape(short) + r'\s*[,}][^{}]*\}\s*=',
        re.S,
    )
    hits = sum(
        len(dot.findall(prod[p]))
        + (
            len(destr.findall(prod[p])) + len(pattern_let.findall(prod[p]))
            if re.search(r'\b' + re.escape(owner) + r'\b', prod[p])
            else 0
        )
        for p in files
        if scope is None or file_crate.get(p, "") in scope
    )
    if hits == 0:
        unread_vf[key] = home

for n in sorted(set(unread_vf) - set(listed)):
    print("UNLISTED\t%s\t%s" % (n[0], n[1]))
for n in sorted(k for k in listed if "::" in k[0] and "." in k[0] and k not in unread_vf):
    print("STALE\t%s\t%s\tnow read, or gone" % (n[0], n[1]))

# Public types nothing uses (the fourth path): a vocabulary the platform forgot to use, or a second
# answer to a question something else already answers.
type_decl = re.compile(r'^\s*pub (?:struct|enum|trait|type) ([A-Z]\w*)', re.M)
type_home = {}
for p in files:
    for m in type_decl.finditer(prod[p]):
        type_home.setdefault((m.group(1), os.path.relpath(p, core)), os.path.relpath(p, core))

unread_t = {}
for (name, rel), home in type_home.items():
    if count_refs(name, home) <= 1:
        unread_t[(name, rel)] = rel

for n in sorted(set(unread_t) - set(listed)):
    print("UNLISTED\t%s\t%s" % (n[0], n[1]))
for n in sorted(k for k in listed if k[0][:1].isupper() and not k[0].isupper() and "::" not in k[0] and "." not in k[0] and k not in unread_t):
    print("STALE\t%s\t%s\tnow used, or gone" % (n[0], n[1]))

# Public constants nothing reads (the fifth path): a value expected to be in force.
const_decl = re.compile(r'^\s*pub (?:const|static) ([A-Z_][A-Z0-9_]*)\s*:', re.M)
const_home = {}
for p in files:
    for m in const_decl.finditer(prod[p]):
        const_home.setdefault((m.group(1), os.path.relpath(p, core)), p)

unread_c = {}
for (name, rel), home in const_home.items():
    if count_refs(name, home) <= 1:
        unread_c[(name, rel)] = rel

print("SCANNED\t%d" % len(files))
for n in sorted(set(unread_c) - set(listed)):
    print("UNLISTED\t%s\t%s" % (n[0], n[1]))
for n in sorted(k for k in listed if k[0].isupper() and "::" not in k[0] and "." not in k[0] and k not in unread_c):
    print("STALE\t%s\t%s\tnow read, or gone" % (n[0], n[1]))
PYEOF
)"

    local swept
    swept="$(printf '%s\n' "$report" | awk -F'\t' '$1=="SCANNED"{print $2}')"
    report="$(printf '%s\n' "$report" | grep -v '^SCANNED' || true)"

    if [ -z "$report" ]; then
        record "PASS" "Unread declarations" "every public function, variant, field and type is used, or listed with a reason" "${swept:-0}"
    else
        local n
        n="$(printf '%s\n' "$report" | wc -l)"
        record "FAIL" "Unread declarations" "${n} declaration(s) unlisted, stale or listed twice"
        printf '%s\n' "$report" | head -30 | sed 's/^/        /'
    fi
}

gate_venue_references() {
    # Every venue adapter has an integration page citing the documents it was built from, with dates.
    # Checked by presence and shape: a reference with no URL cites nothing, and no date is a claim
    # about no particular time.
    local dir="${CORE}/crates/adapters"
    if [ ! -d "$dir" ]; then
        record "NOT RUN" "Venue references" "no adapter directory"
        return
    fi

    # An adapter is a directory holding a venue's own code. Contract, normaliser, profile and registry are shared.
    # The simulated venue is no venue: it has no venue documents to cite, only its own matching rules.
    # The socket is the websocket every adapter shares, no venue's own.
    local shared="contract normalise profile registry simulated socket"
    local missing="" checked=0 name ref
    local adapter
    for adapter in "$dir"/*/; do
        name="$(basename "$adapter")"
        case " $shared " in *" $name "*) continue ;; esac

        ref="${CORE}/docs/integrations/${name}.md"
        if [ ! -r "$ref" ]; then
            missing="${missing} ${name}:absent"
            continue
        fi
        if ! grep -q 'https\?://' "$ref"; then
            missing="${missing} ${name}:no-url"
            continue
        fi
        if ! grep -qE '[Rr]ead [0-9]{4}-[0-9]{2}-[0-9]{2}|\| [0-9]{4}-[0-9]{2}-[0-9]{2}' "$ref"; then
            missing="${missing} ${name}:no-date"
            continue
        fi
        checked=$((checked + 1))
    done

    if [ -n "$missing" ]; then
        record "FAIL" "Venue references" "adapter(s) without a usable reference:${missing}"
    else
        record "PASS" "Venue references" "${checked} adapter(s), each citing its sources with a date" "$checked"
    fi
}

gate_comment_budget() {
    # A run of more than MAX_COMMENT_RUN comment lines is an essay, not a why. The limit is on the run,
    # not a ratio, which would push out one-line whys. The governing document is referred to by position
    # so the identity gate does not trip on it. Rust only: Python docstrings are interface, not comment.
    local max_run=8
    local offenders
    offenders="$(find "${CORE}" -name '*.rs' -not -path '*/target/*' -print0 2>/dev/null \
        | xargs -0 -r awk -v max="$max_run" '
            FNR == 1 { run = 0; start = 0 }
            /^[[:space:]]*(\/\/\/|\/\/!|\/\/)/ {
                if (run == 0) start = FNR
                run++
                next
            }
            {
                if (run > max) printf "%s:%d:%d\n", FILENAME, start, run
                run = 0
            }
            END { if (run > max) printf "%s:%d:%d\n", FILENAME, start, run }
        ')"

    if [ -n "$offenders" ]; then
        local n
        n="$(printf '%s\n' "$offenders" | wc -l)"
        record "FAIL" "Comment budget" "${n} comment run(s) longer than ${max_run} lines"
        printf '%s\n' "$offenders" | sort -t: -k3 -rn | head -5 \
            | sed 's|^'"${REPO_ROOT}"'/||' | sed 's/^/        /'
    else
        record "PASS" "Comment budget" "no comment run longer than ${max_run} lines" "$RUST_SOURCES"
    fi
}

gate_comment_punctuation() {
    # CLAUDE.md 2.8: no semicolon in a comment, a period, comma or colon instead. A ratchet: the count of
    # comment lines holding one may fall and never rise, and a fall lowers the recorded count in the same
    # change, so a cleanup cannot be undone unseen. Semicolons that are code (in strings or after code on
    # the line) are not comments and are not counted.
    local baseline_file="${CORE}/scripts/ci/comment_semicolons.txt"
    if [ ! -r "$baseline_file" ]; then
        record "NOT RUN" "Comment punctuation" "no recorded count alongside this runner"
        return
    fi
    local baseline count
    baseline="$(grep -v '^#' "$baseline_file" | grep -m1 -oE '[0-9]+')"
    count="$(cd "${CORE}" && git ls-files -z -- '*.rs' '*.py' '*.sh' '*.js' '*.css' \
        | xargs -0 -r awk '
            FILENAME ~ /\.(rs|js)$/ && /^[[:space:]]*\/\/.*;/ { n++ }
            FILENAME ~ /\.css$/ && /^[[:space:]]*(\/\*|\*).*;/ { n++ }
            FILENAME ~ /\.(py|sh)$/ && /^[[:space:]]*#[^!].*;/ { n++ }
            END { print n + 0 }
        ')"
    if [ "$count" -gt "$baseline" ]; then
        record "FAIL" "Comment punctuation" "${count} comment line(s) with a semicolon, above the recorded ${baseline}: use a period, a comma or a colon"
    elif [ "$count" -lt "$baseline" ]; then
        record "FAIL" "Comment punctuation" "${count} comment line(s) with a semicolon, below the recorded ${baseline}: lower ${baseline_file##*/} to ${count}"
    else
        record "PASS" "Comment punctuation" "${count} comment line(s) with a semicolon, none added" "$count"
    fi
}

gate_commit_attribution() {
    # CLAUDE.md 2.1 and 8.4: a commit names its authorised identity and nothing else, so no co-author
    # trailer and no tool credit. Every commit the checkout holds is read: a shallow CI checkout reads
    # the pushed tip, a local run the whole branch.
    local examined found
    if ! examined="$(git -C "${CORE}" rev-list --count HEAD 2>/dev/null)" || [ "$examined" -eq 0 ]; then
        record "NOT RUN" "Commit attribution" "no commit history in this checkout"
        return
    fi
    found="$(git -C "${CORE}" log -i -E --format=%h \
        --grep='^co-authored-by:' --grep='generated with \[' HEAD)"
    if [ -n "$found" ]; then
        record "FAIL" "Commit attribution" "$(printf '%s\n' "$found" | grep -c .) commit(s) carry a co-author or tool credit: $(printf '%s' "$found" | head -5 | tr '\n' ' ')" "$examined"
    else
        record "PASS" "Commit attribution" "no co-author trailer or tool credit in any commit" "$examined"
    fi
}

gate_test_layout() {
    # Tests live beside the source tree, never inside it: a reader of src/ reads the product only, and
    # a test module inside a source file is compiled from the file that it is meant to check.
    local sources offenders
    sources="$(count_files "${CORE}" -path '*/src/*' \( -name '*.rs' -o -name '*.py' \) \
        -not -path '*/target/*' -not -path '*/.venv/*' -not -path '*/__pycache__/*')"
    offenders="$(find "${CORE}" -path '*/src/*' -not -path '*/target/*' -not -path '*/.venv/*' \
        \( -name 'tests.rs' -o -name '*_tests.rs' -o -name 'test_*.py' -o -name '*_test.py' \
        -o -name 'conftest.py' -o \( -type d -name tests \) \) -print 2>/dev/null)"
    offenders+="$(find "${CORE}" -path '*/src/*' -name '*.rs' -not -path '*/target/*' -print0 2>/dev/null \
        | xargs -0 -r awk '
            /^[[:space:]]*#\[cfg\(test\)\]/ { armed = 1; next }
            armed && /^[[:space:]]*#\[/ { next }
            armed && /^[[:space:]]*(pub(\([a-z]+\))? )?mod [a-z_0-9]+ *\{/ { printf "\n%s:%d", FILENAME, FNR }
            { armed = 0 }
        ')"
    offenders="$(printf '%s\n' "$offenders" | grep . || true)"

    if [ -n "$offenders" ]; then
        record "FAIL" "Test layout" "$(printf '%s\n' "$offenders" | wc -l) test file(s) or inline test module(s) under src/"
        printf '%s\n' "$offenders" | head -5 | sed 's|^'"${REPO_ROOT}"'/||' | sed 's/^/        /'
    else
        record "PASS" "Test layout" "no test file or inline test module under src/" "$sources"
    fi
}

gate_unit_size() {
    # A file over MAX_FILE lines or a function over MAX_FN lines has more than one reason to change.
    # Test sources, under tests/, are exempt: a table of cases is long by nature. A name is not a
    # test: `backtest.rs` is source. Braces are counted per line, which
    # a brace inside a string literal can mislead. The limit is a review trigger, not a parser.
    local max_file=600 max_fn=80
    local offenders
    offenders="$(find "${CORE}/crates" -name '*.rs' -not -path '*/target/*' -not -path '*/tests/*' \
            -print0 2>/dev/null \
        | xargs -0 -r awk -v max_file="$max_file" -v max_fn="$max_fn" '
            FNR == 1 { if (prev != "" && lines > max_file) printf "%s: %d lines\n", prev, lines; in_fn = 0 }
            { prev = FILENAME; lines = FNR }
            !in_fn && /^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?(const[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+[A-Za-z_]/ {
                in_fn = 1; depth = 0; opened = 0; start = FNR
                name = $0; sub(/^.*fn[[:space:]]+/, "", name); sub(/[^A-Za-z0-9_].*$/, "", name)
            }
            in_fn {
                line = $0
                depth += gsub(/\{/, "{", line) - gsub(/\}/, "}", line)
                if (index($0, "{") > 0) opened = 1
                if (opened && depth <= 0) {
                    if (FNR - start + 1 > max_fn) printf "%s:%d: %s is %d lines\n", FILENAME, start, name, FNR - start + 1
                    in_fn = 0
                } else if (!opened && $0 ~ /;[[:space:]]*$/) {
                    in_fn = 0
                }
            }
            END { if (lines > max_file) printf "%s: %d lines\n", prev, lines }
        ')"

    if [ -n "$offenders" ]; then
        record "FAIL" "Unit size" "$(printf '%s\n' "$offenders" | wc -l) unit(s) over ${max_file}-line files or ${max_fn}-line functions"
        printf '%s\n' "$offenders" | head -8 | sed 's|^'"${REPO_ROOT}"'/||' | sed 's/^/        /'
    else
        record "PASS" "Unit size" "no file over ${max_file} lines, no function over ${max_fn}" "$RUST_SOURCES"
    fi
}

gate_adapter_conformance() {
    # An adapter that implements a venue trait runs the shared conformance checks for it in its own
    # tests, so a new adapter cannot reach CI without its contract being checked. A missing run is
    # named in adapter_conformance_gaps.txt. An entry whose run now exists is stale and fails, so the
    # list only shrinks.
    local gaps="${CORE}/scripts/ci/adapter_conformance_gaps.txt"
    if [ ! -r "$gaps" ]; then
        record "NOT RUN" "Adapter conformance" "no gap list alongside this runner"
        return
    fi

    local problems="" examined=0 dir crate pair trait check covered listed
    local tab=$'\t'
    for dir in "${CORE}"/crates/adapters/*/; do
        crate="$(basename "$dir")"
        [ "$crate" = "registry" ] && continue
        for pair in "MarketFeed:check_feed" "VenueExecution:check_execution"; do
            trait="${pair%%:*}"
            check="${pair##*:}"
            grep -rqE "impl[^{]*\b${trait} for" "${dir}src" 2>/dev/null || continue
            examined=$(( examined + 1 ))
            covered=0
            grep -rqE "\b${check}\(" "${dir}tests" 2>/dev/null && covered=1
            listed=0
            grep -qE "^${crate}${tab}${check}${tab}" "$gaps" && listed=1
            if [ "$covered" = 0 ] && [ "$listed" = 0 ]; then
                problems+="
        ${crate} implements ${trait} and its tests never run ${check}"
            elif [ "$covered" = 1 ] && [ "$listed" = 1 ]; then
                problems+="
        ${crate} is listed as lacking ${check} and now runs it: remove the entry"
            fi
        done
    done

    local entry_crate entry_check _reason
    while IFS="$tab" read -r entry_crate entry_check _reason; do
        case "$entry_crate" in ''|'#'*) continue ;; esac
        [ -d "${CORE}/crates/adapters/${entry_crate}" ] \
            || problems+="
        ${entry_crate} is listed in the gap list and does not exist"
    done < "$gaps"

    if [ -n "$problems" ]; then
        record "FAIL" "Adapter conformance" "$(printf '%s' "$problems" | grep -c .) adapter(s) out of step with the gap list"
        printf '%s\n' "$problems" | head -8
    else
        record "PASS" "Adapter conformance" "every adapter runs its conformance checks or names the gap" "$examined"
    fi
}

gate_tests() {
    local detail="" ok=1 suites=0 out n

    for crate in $(rust_crates); do
        suites=$(( suites + 1 ))
        if out="$(cd "${CORE}/${crate}" && cargo test --quiet --offline 2>&1)"; then
            # Summed over every test binary: head -1 reported the lib count alone and understated coverage.
            n="$(echo "$out" | grep -oP '\d+(?= passed)' | paste -sd+ - | bc 2>/dev/null)"
            detail+="${crate}:${n:-?} "
        else
            detail+="${crate}:FAILED "
            ok=0
            # Kept so a failure that does not repeat can still be read.
            { echo "--- ${crate}"; printf '%s\n' "$out" | tail -n 24; } >&2
        fi
    done

    if [ -z "$PYTHON" ]; then
        record "FAIL" "Tests" "no interpreter at or above the declared floor (${PYTHON_FLOOR:-unknown})"
        return
    fi

    if [ ! -x "$PYTHON" ]; then
        record "NOT RUN" "Tests" "python environment absent; suites unverified"
        return
    fi

    for pkg in $(python_packages); do
        suites=$(( suites + 1 ))
        if out="$(cd "${CORE}/${pkg}" && "$PYTHON" -m pytest -q 2>&1)"; then
            detail+="${pkg}:$(echo "$out" | tail -1 | tr -d '\n') "
        else
            detail+="${pkg}:FAILED "
            ok=0
        fi
    done

    [ "$ok" = 1 ] && record "PASS" "Tests" "$detail" "$suites" || record "FAIL" "Tests" "$detail"
}

gate_boundary() {
    # The core-runs-alone rule: the core starts, ingests and runs with the opt-in programs stopped. the deployment kit acceptance criterion 1:
    # a real drill with the programs an operator would run, a real store and recorded venue traffic.
    if out="$(BOUNDARY_DRILL_SECONDS="${BOUNDARY_DRILL_SECONDS:-8}" \
              bash "${CORE}/scripts/verify/boundary.sh" 2>&1)"; then
        record "PASS" "Boundary" "$(echo "$out" | tail -1 | sed 's/^PASS  //')" "$(drill_steps "$out")"
    else
        record "FAIL" "Boundary" "$(echo "$out" | grep '^FAIL' | head -1 | sed 's/^FAIL  //')"
        echo "$out" | sed 's/^/        /' | head -8
    fi
}



gate_safe_interruption() {
    # The deployment kit criterion 4 and the fail-closed rule: a restart must not bring the order path up before limit state has
    # replayed. Halted is the resting value. Structural claims are the ones worth drilling.
    if out="$(bash "${CORE}/scripts/verify/interrupted_start.sh" 2>&1)"; then
        record "PASS" "Safe interruption" "$(echo "$out" | tail -1 | sed 's/^PASS  //')" "$(drill_steps "$out")"
    else
        record "FAIL" "Safe interruption" "$(echo "$out" | grep '^FAIL' | head -1 | sed 's/^FAIL  //')"
        echo "$out" | sed 's/^/        /' | head -8
    fi
}

gate_recovery_drill() {
    # Recovery: a drill that cannot fail is a demonstration. The drill proves its own refusals. This gate
    # makes sure it is invoked at all.
    if out="$(bash "${CORE}/scripts/verify/restore_drill.sh" 2>&1)"; then
        record "PASS" "Recovery drill" "$(echo "$out" | tail -1 | sed 's/^PASS  //')" "$(drill_steps "$out")"
    else
        record "FAIL" "Recovery drill" "$(echo "$out" | grep '^FAIL' | head -1 | sed 's/^FAIL  //')"
        echo "$out" | sed 's/^/        /' | head -8
    fi
}

gate_keystore_drill() {
    # Access control and the platform architecture, exercised: the resolution path works and the seal refuses a wrong passphrase.
    if out="$(bash "${CORE}/scripts/verify/keystore_drill.sh" 2>&1)"; then
        record "PASS" "Keystore drill" "$(echo "$out" | tail -1 | sed 's/^PASS  //')" "$(drill_steps "$out")"
    else
        record "FAIL" "Keystore drill" "$(echo "$out" | grep '^FAIL' | head -1 | sed 's/^FAIL  //')"
        echo "$out" | sed 's/^/        /' | head -8
    fi
}

gate_third_party_notices() {
    # Release_Gate.md L1: an Open Source Notices surface and THIRD_PARTY_LICENSES, enforced by build check.
    # MIT and Apache-2.0 require their notice to accompany redistribution. A diff, because the generator
    # is reproducible, so a hand-edited notices file is detectable.
    local shipped="${REPO_ROOT}/THIRD_PARTY_LICENSES.md"
    local attest="${CORE}/scripts/ci/licence_attestation.tsv"
    local kit="${CORE}/crates/deployment"
    local registry="${CARGO_HOME:-$HOME/.cargo}/registry/src"

    if [ ! -r "$shipped" ]; then
        record "FAIL" "Third-party notices" "no THIRD_PARTY_LICENSES file is shipped"
        return
    fi
    if [ ! -d "$registry" ]; then
        # Regenerating needs licence texts from the build host's registry. Reported, not passed.
        record "NOT RUN" "Third-party notices" "no package registry to read licence texts from"
        return
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        record "NOT RUN" "Third-party notices" "no cargo to run the generator with"
        return
    fi

    local fresh
    fresh="$(mktemp)"
    if ! out="$(cd "$kit" && cargo run --quiet --offline --bin bijective-deploy -- \
                     notices "$attest" "$registry" "$fresh" 2>&1)"; then
        record "FAIL" "Third-party notices" "$(echo "$out" | tail -1)"
        rm -f "$fresh"
        return
    fi

    if diff -q "$shipped" "$fresh" >/dev/null 2>&1; then
        record "PASS" "Third-party notices" "$(grep -c '^| ' "$shipped") row(s) match the attestation" "$(grep -c '^| ' "$shipped")"
    else
        record "FAIL" "Third-party notices" "the shipped notices differ from what the attestation generates"
        diff "$shipped" "$fresh" | head -6 | sed 's/^/        /'
    fi
    rm -f "$fresh"
}

gate_source_notices() {
    # Apache-2.0 4(c) obliges a redistributor to keep the notices a source file carries. A file that
    # carries none leaves nothing to keep. Checked on the first four lines, where the notice sits.
    local sources=0 offenders="" file head_lines
    while IFS= read -r file; do
        sources=$((sources + 1))
        head_lines="$(head -n 4 "${CORE}/${file}")"
        if ! grep -q 'SPDX-License-Identifier: Apache-2\.0' <<<"$head_lines" \
            || ! grep -qE 'Copyright [0-9]{4} Bijective Labs$' <<<"$head_lines"; then
            offenders+="${file}"$'\n'
        fi
    done < <(git -C "${CORE}" ls-files -- '*.rs' '*.py' '*.sh' ':(exclude)pro/**')

    if [ "$sources" -eq 0 ]; then
        record "FAIL" "Source notices" "no tracked source file was found to examine"
    elif [ -n "$offenders" ]; then
        record "FAIL" "Source notices" "$(printf '%s' "$offenders" | grep -c .) file(s) without the licence and copyright notice"
        printf '%s' "$offenders" | head -5 | sed 's/^/        /'
    else
        record "PASS" "Source notices" "every tracked source file carries the licence and copyright notice" "$sources"
    fi
}

gate_pro_boundary() {
    # The open core is complete without pro/ (CLAUDE.md 2.13 L1): nothing outside it names
    # that directory, so the public export can drop it whole, and every Bijective Pro source says it
    # is not Apache-2.0, so no file crosses the line unmarked in either direction.
    local reach="" unmarked="" file head_lines examined searched
    # The open-core files searched count as examined, so the public export, which holds no
    # Bijective Pro file, still states what it checked.
    searched="$(git -C "${CORE}" ls-files -- ':(exclude)pro/**' | grep -c . || true)"
    examined="$searched"
    # The workspace manifest, its lock and the area list name a Bijective Pro crate so the gates
    # judge it. The public export removes those lines with the directory. A path starting pro/
    # is matched, not a word ending in it (repro/).
    reach="$(git -C "${CORE}" grep -lE '(^|[^A-Za-z0-9_.-])pro/' -- ':(exclude)pro/**' ':(exclude)*.md' \
        ':(exclude)scripts/ci/run_gates.sh' ':(exclude).gitignore' ':(exclude)Cargo.toml' \
        ':(exclude)Cargo.lock' ':(exclude)scripts/ci/areas.txt' || true)"
    while IFS= read -r file; do
        [ -n "$file" ] || continue
        examined=$((examined + 1))
        head_lines="$(head -n 4 "${CORE}/${file}")"
        if grep -q 'SPDX-License-Identifier: Apache-2\.0' <<<"$head_lines" \
            || ! grep -qE 'Copyright [0-9]{4} Bijective Labs\. All rights reserved\.' <<<"$head_lines"; then
            unmarked+="${file}"$'\n'
        fi
    done < <(git -C "${CORE}" ls-files -- 'pro/*.js' 'pro/*.mjs' 'pro/*.css' \
        'pro/*.html' 'pro/*.sh' 'pro/*.rs' 'pro/*.py' 'pro/*.toml')

    # The export, rehearsed: the tracked tree without pro/ and without the lines naming
    # it must still resolve as a workspace. With no open crate depending on a Bijective Pro one
    # (Dependency direction), the open crates then build exactly as they do here.
    local alone="" scratch
    scratch="$(mktemp -d)"
    git -C "${CORE}" ls-files -z -- ':(exclude)pro/**' \
        | (cd "${CORE}" && xargs -0 cp --parents -t "$scratch" 2>/dev/null)
    sed -i '\|"pro/|d' "$scratch/Cargo.toml"
    sed -i '/^pro\//d' "$scratch/scripts/ci/areas.txt"
    if ! (cd "$scratch" && cargo metadata --offline --format-version 1 >/dev/null 2>"$scratch/.err"); then
        alone="$(head -3 "$scratch/.err")"
    fi
    rm -rf "$scratch"

    if [ -n "$alone" ]; then
        record "FAIL" "Pro boundary" "the open core does not resolve without pro/"
        printf '%s\n' "$alone" | sed 's/^/        /'
    elif [ -n "$reach" ]; then
        record "FAIL" "Pro boundary" "the open core names pro/"
        printf '%s\n' "$reach" | head -5 | sed 's/^/        /'
    elif [ -n "$unmarked" ]; then
        record "FAIL" "Pro boundary" "$(printf '%s' "$unmarked" | grep -c .) Bijective Pro file(s) without the proprietary notice"
        printf '%s' "$unmarked" | head -5 | sed 's/^/        /'
    else
        record "PASS" "Pro boundary" "the open core names no pro/ path and resolves without it, every Bijective Pro source is marked" "$examined"
    fi
}

gate_no_required_datastore() {
    # No component carries a cache or database client in its required dependencies.
    # A relational store stays optional behind a port. Optional extras and dev-dependencies are not scanned.
    local clients='redis|fred|rustis|deadpool-redis|valkey|sqlx|tokio-postgres|postgres|diesel|rusqlite|mongodb|pg8000|psycopg|sqlalchemy|pymongo'
    local hits="" examined=0
    local manifest section found

    while IFS= read -r manifest; do
        case "$manifest" in *"/target/"*|*"/.venv/"*) continue ;; esac

        case "$manifest" in
            *Cargo.toml)
                section="$(sed -n '/^\[dependencies\]/,/^\[/p' "$manifest" | grep -vE '^\[|^#')"
                ;;
            *pyproject.toml)
                # One requirement per line whatever the formatting: anchoring to line start would pass a required
                # driver in a single-line list. Quotes are stripped by octal so no quoting is nested here.
                section="$(sed -n '/^dependencies = \[/,/\]/p' "$manifest" \
                    | tr ',[]' '\n\n\n' | tr -d '\042\047' | sed 's/^[[:space:]]*//')"
                ;;
            *) continue ;;
        esac

        examined=$(( examined + 1 ))
        found="$(printf '%s\n' "$section" | grep -inE "^(${clients})([[:space:]]*[=<>~!]|$)" || true)"
        if [ -n "$found" ]; then
            hits="${hits}
        ${manifest#"${CORE}/"}: $(printf '%s\n' "$found" | head -1 | cut -c1-60)"
        fi
    done < <(find "${CORE}" \( -name Cargo.toml -o -name pyproject.toml \) \
                 -not -path '*/target/*' -not -path '*/.venv/*' | sort)

    if [ -n "$hits" ]; then
        record "FAIL" "No required datastore" "a component requires a cache or database client"
        printf '%s\n' "$hits"
    else
        record "PASS" "No required datastore" "the event store is the durable state; adapters stay optional" "$examined"
    fi
}

gate_dependency_direction() {
    # The architecture's dependency direction, enforced: the platform architecture (the core belongs to no
    # engine) and the core-runs-alone rule (nothing in the core may reach into the operational side).
    if [ ! -x "$PYTHON" ]; then
        record "NOT RUN" "Dependency direction" "no interpreter to read the manifests with"
        return
    fi

    local out
    out="$("$PYTHON" - "$CORE" <<'PYEOF'
import pathlib, re, sys

root = pathlib.Path(sys.argv[1])
owner = {}
for m in root.rglob("Cargo.toml"):
    if "target" in m.parts or m.parent == root:
        continue
    found = re.search(r'^name = "([^"]+)"', m.read_text(), re.M)
    if found:
        owner[found.group(1)] = m.parent.relative_to(root).as_posix()

AREAS = {}
for line in (root / "scripts" / "ci" / "areas.txt").read_text().splitlines():
    fields = line.split()
    if len(fields) == 2 and not line.startswith("#"):
        AREAS[fields[0]] = fields[1]

def area(path):
    return AREAS.get(path, "undeclared")

ENGINES = {"data", "control", "strategy"}

# operational may reach anything: it consumes the platform and nothing consumes it.
ALLOWED = {
    "core": {"core"},
    "adapters": {"adapters", "core"},
    "operational": {"core", "adapters", "data", "control", "strategy", "operational"},
    # Bijective Pro components extend the open core through its public interfaces. No open area
    # lists "pro", so an edge from the open core into one is refused (CLAUDE.md 2.13 L1).
    "pro": {"core", "adapters", "data", "control", "strategy", "operational", "pro"},
}

def permitted(frm, to):
    # An engine may reach its own crates, the adapters it ingests through, and the core.
    # Never another engine: that is the fault propagation the process cut exists to prevent.
    if frm in ENGINES:
        return to in {frm, "adapters", "core"}
    return to in ALLOWED.get(frm, set())

bad = []
checked = 0
for m in sorted(root.rglob("Cargo.toml")):
    if "target" in m.parts or m.parent == root:
        continue
    text = m.read_text()
    path = m.parent.relative_to(root).as_posix()
    if "[dependencies]" not in text:
        continue
    section = text.split("[dependencies]")[1].split("\n[")[0]
    for dep in re.findall(r'^(bijective-[a-z-]+)\s*=', section, re.M):
        if dep not in owner:
            continue
        checked += 1
        frm, to = area(path), area(owner[dep])
        if not permitted(frm, to):
            bad.append(f"{path} ({frm}) -> {owner[dep]} ({to})")

print(f"CHECKED {checked}")
for b in bad:
    print(f"AGAINST {b}")
PYEOF
)"

    local checked against
    checked="$(echo "$out" | grep -m1 '^CHECKED' | awk '{print $2}')"
    against="$(echo "$out" | grep -c '^AGAINST' || true)"

    if [ "${against:-0}" -gt 0 ]; then
        record "FAIL" "Dependency direction" "${against} edge(s) against the architecture's direction"
        echo "$out" | grep '^AGAINST' | head -5 | sed 's/^AGAINST /        /'
    else
        record "PASS" "Dependency direction" "${checked:-0} internal edge(s), every one permitted" "${checked:-0}"
    fi
}

gate_order_path_closed() {
    # Nothing reaches a venue unchecked. `VenueExecution::submit` takes a `Clearance` with no public
    # constructor. This pattern finds call sites that mint one, and there is exactly one, in the Control
    # Engine. Not in `limits`: risk limits keeps that crate pure and `Pre-trade check bound` refuses it any dependency.
    local minted
    minted="$(grep -rn 'Clearance::granted' "${CORE}" --include=*.rs 2>/dev/null \
        | grep -v '/target/' \
        | grep -v '/tests.rs:' \
        | grep -v '/tests/' \
        || true)"

    local sites
    sites="$(echo "$minted" | grep -c . || true)"

    local outside
    outside="$(echo "$minted" \
        | grep -v "^${CORE}/crates/model/src/lib.rs:" \
        | grep -v "^${CORE}/crates/control_engine/src/lib.rs:" \
        || true)"

    if [ -n "$outside" ]; then
        record "FAIL" "Order path closed" "a clearance is minted outside the risk evaluation"
        echo "$outside" | head -4 | sed "s|^${CORE}/|        |"
        return
    fi

    # A submit that took no clearance would compile for anyone. Checked because the signature is
    # the whole mechanism, and a later edit could quietly drop it.
    if ! grep -q 'clearance: &.*Clearance' "${CORE}/crates/venue/src/contract/mod.rs" 2>/dev/null; then
        record "FAIL" "Order path closed" "submit no longer requires a clearance"
        return
    fi

    # The flatten clearance (decision 0010) is minted only where a flatten order is judged on the record.
    local reductions
    reductions="$(grep -rn 'Clearance::for_operator_reduction' "${CORE}" --include=*.rs 2>/dev/null \
        | grep -v '/target/' | grep -v '/tests/' \
        | grep -v "^${CORE}/crates/control_engine/src/path/flatten.rs:" \
        || true)"
    if [ -n "$reductions" ]; then
        record "FAIL" "Order path closed" "a flatten clearance is minted outside the flatten order path"
        echo "$reductions" | head -4 | sed "s|^${CORE}/|        |"
        return
    fi

    record "PASS" "Order path closed" "submit requires a clearance; ${sites:-0} mint site, in the Control Engine" "${sites:-0}"
}


gate_determinism() {
    # The same input replayed twice gives byte-identical output.
    # The Tests gate already runs it. It is named here so it stays visible, with two guards against passing vacuously.
    local suite="${CORE}/crates/determinism"
    if [ ! -d "$suite" ]; then
        record "NOT RUN" "Determinism" "no determinism harness in the core"
        return
    fi

    local out
    # Names go to the harness, not cargo: `cargo test` takes one filter and refuses a second.
    if out="$(cd "$suite" && cargo test --quiet --offline -- \
                  one_sequence_replayed_twice_produces_byte_identical_output \
                  a_different_sequence_produces_different_bytes \
                  moving_an_inputs_time_changes_the_bytes 2>&1)"; then
        local n
        n="$(echo "$out" | grep -oP '\d+(?= passed)' | paste -sd+ - | bc 2>/dev/null)"
        record "PASS" "Determinism" "one sequence replayed twice is byte-identical (${n:-?} checks)" "${n:-0}"
    else
        record "FAIL" "Determinism" "replay is not byte-identical, so nothing above it is reproducible"
        echo "$out" | grep -E 'panicked|assertion|FAILED' | head -4 | sed 's/^/        /'
    fi
}

gate_clock_seam() {
    # The platform architecture: every component reads time only through the clock. Scoped to wall time:
    # `Instant::now` for a deadline is monotonic and does not enter the record, and a gate that fires
    # on correct code gets relaxed before it catches anything.
    local hits
    hits="$(grep -rn 'SystemTime::now' "${CORE}" --include=*.rs 2>/dev/null \
        | grep -v '/target/' \
        | grep -v "^${CORE}/crates/common/src/clock/" \
        | grep -v '/tests.rs:' \
        || true)"

    if [ -n "$hits" ]; then
        record "FAIL" "Clock access" "$(echo "$hits" | wc -l) component(s) read the system clock directly"
        echo "$hits" | head -5 | sed "s|^${CORE}/|        |"
    else
        record "PASS" "Clock access" "wall time is read in one place" "$RUST_SOURCES"
    fi
}

gate_runbooks() {
    # The decision registry: a runbook binds to the release it applies to and to a drill that exercises it. A version
    # mismatch or a missing drill is refused. `Drill: none` needs a reason, because an unstated drill is the refused state.
    local dir="${CORE}/docs/how_to"
    local manifest="${CORE}/crates/deployment/deployment.json"

    if [ ! -d "$dir" ] || [ ! -r "$manifest" ]; then
        record "NOT RUN" "Runbooks" "no runbook directory or no deployment manifest"
        return
    fi

    local expected
    expected="$(grep -o '"platform_version"[[:space:]]*:[[:space:]]*"[^"]*"' "$manifest" \
        | head -1 | sed 's/.*"\([^"]*\)"$/\1/')"
    if [ -z "$expected" ]; then
        record "FAIL" "Runbooks" "the deployment manifest states no platform version to bind to"
        return
    fi

    local checked=0 bad="" book version drill reason
    for book in "$dir"/*.md; do
        case "$(basename "$book")" in index.md) continue ;; esac
        checked=$((checked + 1))

        version="$(grep -m1 '^Platform-Version:' "$book" | sed 's/^Platform-Version:[[:space:]]*//')"
        if [ "$version" != "$expected" ]; then
            bad+="
        $(basename "$book"): bound to '${version:-nothing}', platform is ${expected}"
            continue
        fi

        drill="$(grep -m1 '^Drill:' "$book" | sed 's/^Drill:[[:space:]]*//')"
        if [ -z "$drill" ]; then
            bad+="
        $(basename "$book"): names no drill and does not say why"
        elif [ "$drill" = "none" ]; then
            reason="$(grep -m1 '^No-Drill-Reason:' "$book" | sed 's/^No-Drill-Reason:[[:space:]]*//')"
            [ -z "$reason" ] && bad+="
        $(basename "$book"): declares no drill and gives no reason"
        elif [ ! -e "${CORE}/${drill}" ]; then
            bad+="
        $(basename "$book"): names drill '${drill}', which does not exist"
        fi
    done

    if [ "$checked" -eq 0 ]; then
        record "FAIL" "Runbooks" "the runbook directory holds no runbooks"
    elif [ -n "$bad" ]; then
        record "FAIL" "Runbooks" "$(printf '%s' "$bad" | grep -c .) runbook(s) unbound"
        printf '%s\n' "$bad"
    else
        record "PASS" "Runbooks" "${checked} runbook(s) bound to ${expected} and to a drill that exists" "$checked"
    fi
}

gate_licence() {
    if out="$(bash "${CORE}/scripts/verify/no_engine_linkage.sh" 2>&1)"; then
        record "PASS" "Licence linkage" "no forbidden engine crate in any dependency tree" "$(drill_steps "$out")"
    else
        record "FAIL" "Licence linkage" "$(echo "$out" | head -2 | tr '\n' ' ')"
    fi
}


gate_security() {
    local detail=""
    local secrets
    secrets="$(scan_company -E '(api_key|apikey|secret|password|passwd|private_key|token)[[:space:]]*[:=][[:space:]]*["'"'"'][A-Za-z0-9/+_-]{16,}')"
    [ -n "$secrets" ] && detail+="hard-coded-secret-candidates:$(echo "$secrets" | wc -l) "

    # The no-egress rule: no operator data leaves the operator's infrastructure. Adapters are exempt by name.
    # Patterns name egress constructs, not words: `socket\.` fired on variables and missed `connect_async`.
    # One sandbox file is exempt by name, not by directory: its gates open a loopback listener to prove the run cannot reach it.
    local egress
    egress="$(grep -rIn -E 'reqwest::|connect_async|TcpStream::connect|urllib|http\.client|socket\.socket|requests\.(get|post|put|delete)' \
        "${CORE}" --include=*.rs --include=*.py 2>/dev/null \
        | grep -v '/\.venv/' | grep -v '/target/' | grep -v "${CORE}/crates/adapters/" \
        | grep -v "${CORE}/crates/sandbox/tests/unit/tests.rs:" || true)"
    [ -n "$egress" ] && detail+="egress-candidates:$(echo "$egress" | wc -l) "

    [ -n "$detail" ] && record "FAIL" "Security checks" "$detail" \
        || record "PASS" "Security checks" "no secret literals, no egress paths" "$SOURCE_FILES"
}


gate_gate_inventory() {
    # ci_gates.md lists exactly the gates this runner executes, compared in both directions: a gate with
    # no row is undocumented, a row with no gate is a guarantee nothing keeps. The runner is the source.
    local runner="${CORE}/scripts/ci/run_gates.sh"
    local doc="${CORE}/docs/developer_guide/ci_gates.md"

    if [ ! -r "$runner" ] || [ ! -r "$doc" ]; then
        record "FAIL" "Gate inventory" "the runner or ci_gates.md could not be read"
        return
    fi

    # The name a gate reports under is its first record call, keyed by the function the driver calls: a
    # gate defined and never called does not run, and one called and never defined cannot.
    local executed
    executed="$(awk '
        /^gate_[a-z_0-9]+\(\) \{/ { split($0, a, "("); fn = a[1]; next }
        fn != "" && match($0, /record "(PASS|FAIL|NOT RUN)" "[^"]*"/) {
            s = substr($0, RSTART, RLENGTH)
            sub(/^record "(PASS|FAIL|NOT RUN)" "/, "", s)
            sub(/"$/, "", s)
            if (!(fn in name)) name[fn] = s
        }
        /^gate_[a-z_0-9]+$/ { order[++n] = $0 }
        END {
            for (i = 1; i <= n; i++) {
                print (order[i] in name) ? name[order[i]] : "UNNAMED " order[i]
            }
        }
    ' "$runner" | sort -u)"

    local listed
    listed="$(grep -oE '^\| \*\*[^*]+\*\*' "$doc" | sed 's/^| \*\*//; s/\*\*$//' | sort -u)"

    local problems="" gate_name
    while IFS= read -r gate_name; do
        [ -n "$gate_name" ] || continue
        printf '%s\n' "$listed" | grep -xF "$gate_name" >/dev/null \
            || problems+="
        the runner executes ${gate_name}, which ci_gates.md does not list"
    done <<< "$executed"

    while IFS= read -r gate_name; do
        [ -n "$gate_name" ] || continue
        printf '%s\n' "$executed" | grep -xF "$gate_name" >/dev/null \
            || problems+="
        ci_gates.md lists ${gate_name}, which the runner does not execute"
    done <<< "$listed"

    local counted
    counted="$(printf '%s\n' "$executed" | grep -c . || true)"

    if [ -n "$problems" ]; then
        record "FAIL" "Gate inventory" "$(printf '%s' "$problems" | grep -c .) gate(s) out of step with ci_gates.md"
        printf '%s\n' "$problems" | head -8
    else
        record "PASS" "Gate inventory" "the table lists exactly what this runner executes" "$counted"
    fi
}

gate_release_path() {
    # An artefact can be assembled, recorded and refused when it differs, including that the release
    # carries the Python library. A change gate, not a drill: no delegated cgroup or privilege needed.
    if out="$(bash "${CORE}/scripts/verify/release.sh" 2>&1)"; then
        record "PASS" "Release path" "assembled, recorded, and refused when changed" \
            "$(printf '%s\n' "$out" | grep -c '^PASS')"
    else
        record "FAIL" "Release path" "the release path does not hold"
        printf '%s\n' "$out" | grep -E '^(FAIL|NOT )' | head -4 | sed 's/^/        /'
    fi
}

gate_proposal_streams() {
    # The stream the Control Engine reads is written by a live session and nothing else.
    # The name is the whole isolation, so the name is checked: it may appear only where the live stage maps
    # to it and where the ledger reads entries written before that field existed.
    local streams="${CORE}/crates/serialization/src/signal/node.rs"
    local live
    live="$(sed -n '/^pub const LIVE/,/^};/s/^    proposals: "\(.*\)",$/\1/p' "$streams")"
    if [ -z "$live" ]; then
        record "FAIL" "Proposal streams" "the Control Engine no longer names the stream it reads"
        return
    fi

    local allowed="crates/strategy_engine/src/runs/mod.rs crates/strategy_engine/src/runs/ledger.rs"
    local files stray=0 scanned=0 detail="" rel
    files="$(find $(area_dirs strategy) -name '*.rs' -not -name 'tests.rs' -not -name '*_tests.rs')"
    while IFS= read -r f; do
        [ -n "$f" ] || continue
        scanned=$(( scanned + 1 ))
        rel="${f#"${CORE}/"}"
        case " ${allowed} " in *" ${rel} "*) continue ;; esac
        if grep -q "\"${live}\"" "$f"; then
            stray=$(( stray + 1 ))
            detail="${detail}
        ${rel} names ${live}, the stream that reaches a venue"
        fi
    done <<< "$files"

    if [ "$stray" -gt 0 ]; then
        record "FAIL" "Proposal streams" "${stray} file(s) in the Strategy Engine name the stream that reaches a venue"
        printf '%s\n' "$detail" | head -5
    else
        record "PASS" "Proposal streams" "only a live session writes ${live}" "${scanned}"
    fi
}


gate_replay_parity() {
    # A program's emitted stream is a function of its clock and its store only.
    # A rule nothing could violate is untested, so this drives the two variations made possible.
    if out="$(bash "${CORE}/scripts/verify/replay_parity.sh" 2>&1)"; then
        record "PASS" "Replay parity" "$(echo "$out" | tail -1 | sed 's/^PASS  //')" "$(drill_steps "$out")"
    else
        record "FAIL" "Replay parity" "$(echo "$out" | grep '^FAIL' | head -1 | sed 's/^FAIL  //')"
        echo "$out" | sed 's/^/        /' | head -8
    fi
}

gate_order_path() {
    # A proposal enters, is answered once, and a restart does not answer it again. This is the path a
    # Operator's capital travels, and the resume mark is the property that spends money when it is wrong.
    if out="$(bash "${CORE}/scripts/verify/order_path.sh" 2>&1)"; then
        record "PASS" "Order path" "$(echo "$out" | tail -1 | sed 's/^PASS  //')" "$(drill_steps "$out")"
    else
        record "FAIL" "Order path" "$(echo "$out" | grep '^FAIL' | head -1 | sed 's/^FAIL  //')"
        echo "$out" | sed 's/^/        /' | head -8
    fi
}

gate_credentials() {
    # The platform architecture binding rule 5: no component resolves a credential from configuration or source. Without
    # one from the broker it fails closed. The pattern is the access control shape (a credential-shaped
    # identifier resolved from the environment or a config read), not the word "secret", which names the broker too.
    local cred='api[_-]?key|api[_-]?secret|private[_-]?key|passphrase|password|access[_-]?token|secret[_-]?key'
    local source='env::var|std::env|getenv|os\.environ|environ\[|dotenv|from_config|config\.get|settings\.'
    local hits="" scanned=0

    for dir in $(rust_crates) $(python_packages); do
        [ -d "${CORE}/${dir}/src" ] || continue
        scanned=$(( scanned + 1 ))
        local found
        found="$(grep -rInE "(${cred})" "${CORE}/${dir}/src" 2>/dev/null \
            | grep -IE "(${source})" || true)"
        [ -n "$found" ] && hits="${hits}${found}"$'\n'
    done

    if [ -n "$(printf '%s' "$hits" | tr -d '[:space:]')" ]; then
        record "FAIL" "Credential resolution" "credential read from configuration or environment"
        printf '%s' "$hits" | head -5 | sed 's/^/        /'
    else
        record "PASS" "Credential resolution" "no credential resolved from configuration or environment" "$scanned"
    fi
}

gate_pre_trade_bound() {
    # Risk limits and section 6: the pre-trade check is pure computation over memory with a bounded worst
    # case and no wall clock. Two conditions, because either alone is insufficient: no dependencies still
    # allows filesystem access through std, and avoiding std still allows I/O through a dependency.
    local crate="${CORE}/crates/limits"
    if [ ! -d "${crate}/src" ]; then
        record "NOT RUN" "Pre-trade check bound" "limit crate absent"
        return
    fi

    local deps
    deps="$(sed -n '/^\[dependencies\]/,/^\[/p' "${crate}/Cargo.toml" \
        | grep -vE '^\[|^[[:space:]]*$|^#' || true)"

    # Blocking, I/O-capable or clock-reading facilities. Formatting is excluded: a verdict must be printable.
    local forbidden='std::fs|std::net|std::io|std::process|std::thread|Mutex|RwLock|Condvar|mpsc|SystemTime|Instant::now'
    local reached
    reached="$(grep -rInE "(${forbidden})" "${crate}/src" 2>/dev/null | grep -v '^\s*//' || true)"

    # The decision path allocates nothing. Scoped to the body of `assess`, not the crate:
    # `PreTradeCheck::new` allocates its limit set by design. Blind spot: callees are not read. Tolerable only
    # while `Limit` and `Amount` stay `Copy` arithmetic.
    local allocating='format!|to_string\(|to_owned\(|String::|Vec::|vec!|Box::|\.collect\(|\.clone\(\)'
    local body allocs=""
    body="$(sed -n '/pub fn assess/,/^    }$/p' "${crate}/src/pre_trade.rs" 2>/dev/null || true)"
    if [ -z "$body" ]; then
        record "NOT RUN" "Pre-trade check bound" "assess not found; the allocation check had nothing to read"
        return
    fi
    allocs="$(printf '%s\n' "$body" | grep -nE "(${allocating})" | grep -vE '^\s*[0-9]+:\s*//' || true)"

    if [ -n "$deps" ]; then
        record "FAIL" "Pre-trade check bound" "limit crate declares dependencies; the bound is no longer local"
        printf '%s\n' "$deps" | head -3 | sed 's/^/        /'
    elif [ -n "$reached" ]; then
        record "FAIL" "Pre-trade check bound" "decision path reaches a blocking, I/O or clock facility"
        printf '%s\n' "$reached" | head -5 | sed 's/^/        /'
    elif [ -n "$allocs" ]; then
        record "FAIL" "Pre-trade check bound" "the decision path allocates"
        printf '%s\n' "$allocs" | head -5 | sed 's/^/        assess: /'
    else
        record "PASS" "Pre-trade check bound" "no dependency, no I/O, no lock, no wall clock, no allocation in assess" "$(printf '%s\n' "$body" | grep -c . || true)"
    fi
}

gate_lint() {
    local detail="" ok=1 linted=0
    if ! (cd "${CORE}" && cargo fmt --all --check) >/dev/null 2>&1; then
        detail+="rustfmt "
        ok=0
    fi
    for crate in $(rust_crates); do
        linted=$(( linted + 1 ))
        if ! (cd "${CORE}/${crate}" && cargo clippy --offline --all-targets -- -D warnings) >/dev/null 2>&1; then
            detail+="clippy:${crate} "
            ok=0
        fi
    done
    if [ -x "$PYTHON" ]; then
        for pkg in $(python_packages); do
            linted=$(( linted + 1 ))
            if [ -n "$("$PYTHON" -m pyflakes "${CORE}/${pkg}/src" 2>/dev/null)" ]; then
                detail+="pyflakes:${pkg} "
                ok=0
            fi
        done
    else
        detail+="pyflakes:NOT-RUN "
        ok=0
    fi
    [ "$ok" = 1 ] && record "PASS" "Format and lint" "rustfmt, clippy -D warnings, pyflakes" "$linted" \
        || record "FAIL" "Format and lint" "$detail"
}

echo "Bijective change gates"
echo "========================"
gate_tests
gate_boundary
gate_safe_interruption
gate_recovery_drill
gate_keystore_drill
gate_clock_seam
gate_no_required_datastore
gate_dependency_direction
gate_order_path_closed
gate_determinism
gate_runbooks
gate_licence
gate_security
gate_credentials
gate_pre_trade_bound
gate_structure
gate_no_float
gate_venue_names
gate_order_attribution
gate_dependency_licences
gate_third_party_notices
gate_source_notices
gate_pro_boundary
gate_agent_environment
gate_venue_references
gate_venue_isolation
gate_market_archive
gate_console_tls
gate_unread_declarations
gate_comment_budget
gate_comment_punctuation
gate_commit_attribution
gate_test_layout
gate_unit_size
gate_adapter_conformance
gate_order_path
gate_replay_parity
gate_proposal_streams
gate_release_path
gate_gate_inventory
gate_lint

echo
printf '%-8s %-22s %8s  %s\n' "RESULT" "GATE" "EXAMINED" "DETAIL"
for row in "${results[@]}"; do
    IFS='|' read -r status gate examined detail <<< "$row"
    printf '%-8s %-22s %8s  %s\n' "$status" "$gate" "$examined" "$detail"
done

echo
if [ "$failures" -eq 0 ]; then
    echo "All gates passed."
else
    echo "${failures} gate(s) failed or could not be run."
    echo "A gate that could not run is reported as a failure, not a skip: an unverified"
    echo "change is unverified, and the one case a gate exists to catch is the case a"
    echo "silent skip would wave through."
fi
exit $((failures == 0 ? 0 : 1))
