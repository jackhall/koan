#!/usr/bin/env bash
# Run the koan build-verification slate: instrumented unit tests (cargo
# llvm-cov), doctests (including `compile_fail` guards, which llvm-cov does not
# run), lints, doclinks, tutorial-snippet output checks, the allocation audit,
# and the modgraph fractal-complexity score.
# Mirrors the `verify-koan` skill (.claude/skills/verify-koan/).
#
# One line per step, then one summary line. A step that passes is worth a count,
# a score, or a delta — not its runner chatter — so the whole green slate reads
# without scrolling, and the summary line is the report itself rather than
# something a reader has to assemble from the transcript. A step that fails is
# the only one to get its output back, replayed in full under its own banner.
# Pass `KOAN_VERBOSE=1` to replay every step's output, passing or not.
#
# The modgraph, coverage, and allocation-audit steps print current readings. They
# rebaseline `observe/complexity.txt` / `observe/coverage.txt` / `observe/alloc.txt` only when invoked with
# `KOAN_REBASELINE` set — pre-commit sets it; manual runs leave it unset,
# since the trend logs should record one entry per commit, not one per
# local sanity-check. Either way the reading is reported beside its delta.
#
# Outputs (override paths via env vars):
#   - DOT graph from cargo-modules → observe/modules.dot   (`KOAN_DOT`)
#   - llvm-cov lcov report          → observe/coverage.lcov (`KOAN_LCOV`)
#
# Not run here (too heavy for the per-commit hook, run on demand / in CI):
# `tools/seam_equivalence.sh` — the record-escape-seam equivalence battery, which
# re-runs the suite under `--features seam-force-copy` and `--features seam-force-pin`
# to prove the cost-driven copy-vs-pin choice is semantically invisible.
#
# Two slates, picked by change scope. When every changed path is under
# `workgraph/`, the change is library-side and koan's adoption of the new surface
# is a separate work item, so the library slate runs and koan's compile state is
# reported rather than gated — that is what lets a workgraph-only commit land
# ahead of its koan adoption. Any koan-side change selects the full slate, where
# koan compiling is a gate as usual.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

DOT="${KOAN_DOT:-observe/modules.dot}"
LCOV="${KOAN_LCOV:-observe/coverage.lcov}"
REBASELINE="${KOAN_REBASELINE:-}"
VERBOSE="${KOAN_VERBOSE:-}"

SCOPE=""
CLAUSES=()
OUT=""

# The step line: a fixed-width label, then the one thing the step has to say.
ok() {
    printf '  %-16s %s\n' "$1" "$2"
    CLAUSES+=("$3")
    [ -z "$VERBOSE" ] || detail "$OUT"
}

# Sub-lines under a step: the rows a reading step had left to show, or, under
# KOAN_VERBOSE, the whole of what it said. Nothing to show prints nothing, the
# leading blank a rendered table carries is dropped so the sub-lines sit under
# their step line, and a blank line stays blank rather than becoming indentation.
detail() {
    [ -n "$1" ] || return 0
    printf '%s\n' "$1" | sed -e '/./,$!d' -e '/./s/^/    /'
}

# A failed step closes the summary on the failure and aborts the slate, after
# replaying every line it printed. This is the one place the slate is loud.
fail() {
    printf '  %-16s FAILED\n' "$1"
    CLAUSES+=("$2")
    printf '\n--- %s ---\n%s\n' "$1" "$3"
    summary
    exit 1
}

# The line to report. Every clause is one step's verdict, in slate order.
summary() {
    local line="Verify${SCOPE:+ ($SCOPE)}:" clause
    for clause in "${CLAUSES[@]}"; do line+=" $clause,"; done
    printf '\n%s\n' "${line%,}."
}

# Run a step, capturing its output into OUT. On failure, hand the label, the
# summary clause, and the whole captured output to `fail`, which exits.
run() {
    local label=$1 clause=$2
    shift 2
    OUT="$("$@" 2>&1)" || fail "$label" "$clause" "$OUT"
}

# `test result: ok. N passed; ...`, summed over every test binary.
passed() { awk '/^test result: ok\./ {total += $4} END {print total + 0}' <<<"$OUT"; }

# Both trend-log tools print `<name>: <what> <now> vs prior <prev> from <date>
# <sha> (Δ <d>).`; compact that to `<now> (Δ <d> vs <prev>)`. The first-run and
# no-baseline wordings do not match, and fall through with only the name stripped.
compact() {
    sed -E -e 's/^(coverage|baseline): //' \
           -e 's/^(line |score )?([0-9.]+%?) vs prior ([0-9.]+%?) from [^(]*\(Δ ([-+0-9.]+)(, recorded to [^)]*)?\)\.$/\2 (Δ \4 vs \3)/' \
        <<<"$1"
}

# Every path differing from HEAD — staged, unstaged, and untracked. A clean tree
# yields one empty line, which fails the `workgraph/` case and so selects the full
# slate: with nothing changed there is no library-side commit to unblock.
CHANGED="$(git diff --name-only HEAD; git ls-files --others --exclude-standard)"
WORKGRAPH_ONLY=1
while IFS= read -r path; do
    case "$path" in
        workgraph/*) ;;
        *) WORKGRAPH_ONLY=0 ;;
    esac
done <<<"$CHANGED"

if [ "$WORKGRAPH_ONLY" = 1 ]; then
    SCOPE="workgraph only"
    printf 'Change scope: workgraph only — running the library slate.\n\n'

    # Runs unit tests and doctests in one pass — unlike the full slate, where the
    # doctests need their own step because llvm-cov cannot run them. That covers the
    # `compile_fail` escape guards, which are doctests.
    #
    # `test-hooks` widens the white-box surface koan's own tests reach. It is off in
    # a default build, so compiling it here is what keeps the gated code in the slate.
    run tests 'tests FAILED' \
        cargo test -p workgraph --features test-hooks --quiet
    ok tests "ok ($(passed) passed, unit + doctests)" 'tests ok'

    if OUT="$(cargo clippy -p workgraph --all-targets --features test-hooks -- -D warnings 2>&1)"; then
        ok clippy clean 'clippy clean'
    else
        cargo clippy -p workgraph --fix --allow-dirty --allow-staged \
            --all-targets --features test-hooks >/dev/null 2>&1 || true
        run clippy 'clippy: issues remain after --fix' \
            cargo clippy -p workgraph --all-targets --features test-hooks -- -D warnings
        ok clippy 'clean after --fix (working tree modified)' 'clippy clean after --fix'
    fi

    run doclinks 'doclinks FAILED' python3 tools/doclinks.py check --gates-only
    ok doclinks '4 gates clean' 'doclinks ok'

    # Informational, never gating: koan failing to compile against workgraph HEAD is
    # the expected mid-migration state, and its size is the adoption debt now owed.
    if OUT="$(cargo check -p koan --all-targets 2>&1)"; then
        ok koan 'compiles — no adoption debt' 'koan compiles'
    else
        errors="$(grep -c '^error' <<<"$OUT")"
        ok koan "does NOT compile — $errors errors of adoption debt owed by a koan-side item" \
            "koan does NOT compile — $errors errors of adoption debt"
    fi

    summary
    exit 0
fi

run tests 'tests FAILED' cargo llvm-cov --quiet --lcov --output-path "$LCOV"
ok tests "ok ($(passed) passed → $LCOV)" 'tests ok'

# llvm-cov does not run doctests (instrumented doctests are nightly-only), so the
# `compile_fail` escape guards on the lifetime-erasure accessors go unchecked above.
# Run them here: a `compile_fail` doctest that *starts* compiling is a test failure.
run doctests 'doctests FAILED' cargo test --doc --quiet
ok doctests "ok ($(passed) passed, compile_fail guards included)" 'doctests ok'

if OUT="$(cargo clippy --all-targets -- -D warnings 2>&1)"; then
    ok clippy clean 'clippy clean'
else
    cargo clippy --fix --allow-dirty --allow-staged --all-targets >/dev/null 2>&1 || true
    run clippy 'clippy: issues remain after --fix' cargo clippy --all-targets -- -D warnings
    ok clippy 'clean after --fix (working tree modified)' 'clippy clean after --fix'
fi

# --gates-only drops the informational source-tree changes report; the four
# gating audits (links, deps, orphans, next-items) still run and still gate.
run doclinks 'doclinks FAILED' python3 tools/doclinks.py check --gates-only
ok doclinks '4 gates clean' 'doclinks ok'

# The tutorial's runnable snippets (```koan blocks with an expected ```text output)
# are diffed against the interpreter. Needs the plain debug binary — llvm-cov above
# builds an instrumented one under a different profile, so build it explicitly.
run snippets 'snippets FAILED (build)' cargo build --quiet
run snippets 'snippets FAILED' python3 tools/verify_snippets.py
matched="$(grep -oE '^[0-9]+/[0-9]+ runnable' <<<"$OUT" | cut -d' ' -f1)"
ok snippets "$matched matched" "snippets $matched"

# The allocation shapes in `audit/shapes/`, swept through the counting allocator
# (`--features alloc-count`) and the bounded regression test's brackets. Reports each
# shape's totals, the marginal terms differenced out of them, and the headroom left on
# every bound in `tests/allocation_baseline.rs`. Never gating: the bounds themselves
# are asserted by that test, which the tests step ran. `--quiet` keeps the rows that
# moved against the recorded sweep and the bounds that drifted, dropping the rest.
run alloc 'allocation audit FAILED' \
    python3 tools/alloc_audit.py --quiet ${REBASELINE:+--baseline}
alloc_head="$(head -1 <<<"$OUT")"
ok alloc "${alloc_head#allocation audit: }" "allocation audit ${alloc_head##*; }"
[ -n "$VERBOSE" ] || detail "$(tail -n +2 <<<"$OUT")"

run coverage 'coverage FAILED' python3 tools/coverage.py --lcov "$LCOV" \
    ${REBASELINE:+--baseline observe/coverage.txt}
delta="$(compact "$(grep '^coverage: line' <<<"$OUT" | tail -1)")"
ok coverage "line $delta" "coverage $delta"

run 'modgraph tests' 'modgraph tooling tests FAILED' python3 tools/modgraph/tests.py
ok 'modgraph tests' "ok ($(awk '/^Ran [0-9]+ test/ {print $2}' <<<"$OUT") passed)" \
    'modgraph tests ok'

# `regen` runs cargo-modules, re-attributes uses edges to the written import
# surface (re-export correction), refreshes observe/doc_graph.dot, then scores.
# --quiet drops the per-module report and the regeneration progress, leaving the
# bottom-line score and its delta against the trend log.
run modgraph 'modgraph score FAILED' \
    python3 tools/modgraph regen --root koan --edges "$DOT" --quiet \
    ${REBASELINE:+--baseline observe/complexity.txt}
delta="$(compact "$(grep '^baseline: score' <<<"$OUT" | tail -1)")"
ok modgraph "score $delta" "modgraph score $delta"
[ -n "$VERBOSE" ] || detail "$(grep -oE '\(coupling.*\)$' <<<"$OUT")"

summary
