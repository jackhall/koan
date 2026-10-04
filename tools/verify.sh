#!/usr/bin/env bash
# Run a koan verification slate. Two tiers, and the tier is the argument:
#
#     tools/verify.sh            # routine — the per-commit slate the pre-commit hook runs
#     tools/verify.sh --total    # total   — the deep slate CI runs, and a human runs on demand
#
# The routine tier answers "did this change break anything": tests, doctests, lints, doc links.
# It is meant to cost seconds, so it measures nothing — no coverage instrumentation, no module
# graph, no Miri, no benchmark sweep. The total tier is where the expensive readings live:
# coverage, the fractal complexity score, the Miri leak/UB audit, the cellgraph verb sweep, and a
# property sweep an order of magnitude deeper than the routine one. Only the total tier
# rebaselines the trend logs.
#
# Property depth is one variable, `PROPTEST_CASES`, which `ProptestConfig::default()` reads: the
# routine tier sets 64 and the total tier 2048, and every property module scales off that default
# (`crate::tests::case_share`) so their relative depths hold at either setting. Export it yourself
# to override the tier's choice — `PROPTEST_CASES=16384 tools/verify.sh --total` for an overnight
# sweep of the lattice laws.
#
# Every cargo step builds the default feature set, and every one but the release check builds the
# debug profile. The routine tier runs the tutorial's snippets through the interpreter binary.
#
# One line per step, then one summary line. A step that passes is worth a count, a score, or a
# delta — not its runner chatter — so the whole green slate reads without scrolling, and the
# summary line is the report itself rather than something a reader has to assemble from the
# transcript. A step that fails is the only one to get its output back, replayed in full under its
# own banner. Pass `KOAN_VERBOSE=1` to replay every step's output, passing or not.
#
# Every step's whole output, passing or not, also lands in a log the run starts afresh —
# `scratch/verify.log` (`KOAN_VERIFY_LOG`), named under the summary line — so a failure's detail
# survives however the caller trimmed what it printed, and a question about a passing step is
# answered by reading the log, not by running the slate again.
#
# Outputs, total tier only (override paths via env vars):
#   - DOT graph from cargo-modules → observe/modules.dot   (`KOAN_DOT`)
#   - llvm-cov lcov report          → observe/coverage.lcov (`KOAN_LCOV`)
#     (workspace-wide: koan plus its embedded crates)
#   - cellgraph verb readings       → cellgraph/observe/perf.csv, appended under `KOAN_REBASELINE`
#
# Scope, routine tier only. When every changed path is a Markdown file the change cannot reach a
# build, so the slate is the link audit and nothing else. When every changed path is under one
# embedded crate — `cellgraph/` — the change is library-side and the adoption of
# the new surface above it is a separate work item, so that crate's slate runs and everything above
# it is reported rather than gated — that is what lets a library-only commit land ahead of its
# adoption. Anything else takes the whole workspace, where every crate compiling is a gate as
# usual. The total tier reads no scope: it always runs everything.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

TIER=routine
case "${1:-}" in
    --total) TIER=total ;;
    --routine | '') ;;
    *)
        printf 'usage: tools/verify.sh [--total]\n' >&2
        exit 2
        ;;
esac

DOT="${KOAN_DOT:-observe/modules.dot}"
LCOV="${KOAN_LCOV:-observe/coverage.lcov}"
REBASELINE="${KOAN_REBASELINE:-}"
VERBOSE="${KOAN_VERBOSE:-}"

# The tier's property depth, unless the caller named one.
if [ "$TIER" = total ]; then
    export PROPTEST_CASES="${PROPTEST_CASES:-2048}"
else
    export PROPTEST_CASES="${PROPTEST_CASES:-64}"
fi

# One core held back. `.cargo/config.toml` does this for the compile side (`jobs = -1`);
# this is the run side, where libtest otherwise spawns one thread per available core.
if [ -z "${RUST_TEST_THREADS:-}" ]; then
    CORES="$(nproc)"
    export RUST_TEST_THREADS=$((CORES > 1 ? CORES - 1 : 1))
fi

SCOPE=""
CLAUSES=()
OUT=""

LOG="${KOAN_VERIFY_LOG:-scratch/verify.log}"
mkdir -p "$(dirname "$LOG")"
printf 'Verify, %s tier, at %s, %s property cases.\n' "$TIER" "$(git rev-parse --short HEAD)" \
    "$PROPTEST_CASES" >"$LOG"

# A step's whole output, under its own banner, into the log.
logged() {
    printf '\n--- %s ---\n%s\n' "$1" "$2" >>"$LOG"
}

# The step line: a fixed-width label, then the one thing the step has to say.
ok() {
    printf '  %-16s %s\n' "$1" "$2"
    logged "$1" "$OUT"
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
    logged "$1" "$3"
    CLAUSES+=("$2")
    printf '\n--- %s ---\n%s\n' "$1" "$3"
    summary
    exit 1
}

# The line to report. Every clause is one step's verdict, in slate order.
summary() {
    local line="Verify${SCOPE:+ ($SCOPE)}:" clause
    for clause in "${CLAUSES[@]}"; do line+=" $clause,"; done
    printf '\n%s\n' "${line%,}." | tee -a "$LOG"
    printf 'Full output: %s\n' "$LOG"
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

# The `unsafe`-site drift check against the Miri slate's group anchors. Cheap, and it is what keeps
# the total tier's Miri step honest: a new unsafe site the slate does not name fails here, in the
# routine tier, rather than going unaudited until someone runs the deep slate.
slate_audit() {
    run slate-audit 'miri slate DRIFTED' python3 tools/observe_tests.py slate-audit
    ok slate-audit 'no unsafe-site drift' 'slate ok'
}

# cellgraph's public surface must not depend on the build profile: a public item
# behind a `debug_assertions` gate is an API that exists in one profile and not the
# other, and an embedder that compiles in debug would break in release. Stated two
# ways — sampled, by running the test that names every door again under `--release`
# to catch a door the release build dropped; and directly, by reading the crate's
# source for the gate itself. Only `debug_assert!` may gate on the profile, and it
# expands to the gate rather than writing it.
cellgraph_surface() {
    run surface-release 'surface test FAILED under --release' \
        cargo test -p cellgraph --release --test surface --quiet
    ok surface-release 'holds under --release' 'surface ok under --release'

    OUT="$(grep -rn 'cfg(debug_assertions)' cellgraph/src || true)"
    if [ -z "$OUT" ]; then
        ok profile-free 'no cfg(debug_assertions) under cellgraph/src' 'surface profile-free'
    else
        fail profile-free 'cellgraph/src gates on debug_assertions' "$OUT"
    fi
}

# The release profile drops `cfg(debug_assertions)` items, but `debug_assert!` still type-checks its
# arguments, so a debug-only helper named in a `debug_assert!` builds in debug and breaks in
# release. Every other step builds debug; this one checks the whole workspace under `--release`,
# warnings included, since an import only debug code uses goes unused there.
release_step() {
    run release 'release build FAILED' cargo check --workspace --all-targets --release --quiet
    if grep -q '^warning' <<<"$OUT"; then
        fail release 'release build WARNS' "$OUT"
    fi
    ok release 'checks clean under --release' 'release ok'
}

# clippy, auto-fixing once before it gates. `$@` is the package/feature selection.
clippy_step() {
    if OUT="$(cargo clippy "$@" -- -D warnings 2>&1)"; then
        ok clippy clean 'clippy clean'
    else
        cargo clippy --fix --allow-dirty --allow-staged "$@" >/dev/null 2>&1 || true
        run clippy 'clippy: issues remain after --fix' cargo clippy "$@" -- -D warnings
        ok clippy 'clean after --fix (working tree modified)' 'clippy clean after --fix'
    fi
}

# Every runnable tutorial snippet through the interpreter binary, against the output the tutorial
# shows; a snippet using an expression shape the rewrite does not run yet is skipped.
snippets_step() {
    run snippets 'snippets FAILED' cargo build --quiet --bin koan
    run snippets 'snippets FAILED' python3 tools/verify_snippets.py
    local matched pending
    matched="$(grep -oE '^[0-9]+/[0-9]+' <<<"$OUT")"
    pending="$(grep -oE '[0-9]+ pending' <<<"$OUT")"
    ok snippets "$matched matched, $pending" 'snippets ok'
}

# --gates-only drops the informational source-tree changes report; the four
# gating audits (links, deps, orphans, next-items) still run and still gate.
doclinks_step() {
    run doclinks 'doclinks FAILED' python3 tools/doclinks.py check --gates-only
    ok doclinks '4 gates clean' 'doclinks ok'
}

if [ "$TIER" = routine ]; then
    # Every path differing from HEAD — staged, unstaged, and untracked. A clean tree yields one
    # empty line, which matches no scope and so selects the whole workspace: with nothing changed
    # there is no narrower commit to unblock. A change spanning two scopes clears both flags.
    CHANGED="$(git diff --name-only HEAD; git ls-files --others --exclude-standard)"
    DOCS_ONLY=1
    CELLGRAPH_ONLY=1
    while IFS= read -r path; do
        [ -n "$path" ] || { DOCS_ONLY=0; continue; }
        case "$path" in
            *.md) CELLGRAPH_ONLY=0 ;;
            cellgraph/*) DOCS_ONLY=0 ;;
            *) DOCS_ONLY=0; CELLGRAPH_ONLY=0 ;;
        esac
    done <<<"$CHANGED"

    if [ "$DOCS_ONLY" = 1 ]; then
        SCOPE="docs only"
        printf 'Change scope: Markdown only — running the link audit.\n\n'
        doclinks_step
        summary
        exit 0
    fi

    if [ "$CELLGRAPH_ONLY" = 1 ]; then
        SCOPE="cellgraph only"
        printf 'Change scope: cellgraph only — running the cell-substrate slate.\n\n'

        # The one feature the crate has, `perf`, gates a binary rather than any library code, so
        # the tests see everything without it and only clippy below turns it on.
        run tests 'tests FAILED' cargo test -p cellgraph --quiet
        ok tests "ok ($(passed) passed, unit + doctests)" 'tests ok'

        cellgraph_surface
        clippy_step -p cellgraph --all-targets --features perf
        doclinks_step

        # Informational, never gating: koan failing to compile against cellgraph HEAD is the
        # adoption debt a koan-side item owes.
        if OUT="$(cargo check -p koan --all-targets 2>&1)"; then
            ok koan 'compiles' 'koan compiles'
        else
            errors="$(grep -c '^error' <<<"$OUT")"
            ok koan "does NOT compile — $errors errors" "koan does NOT compile — $errors errors"
        fi

        summary
        exit 0
    fi

    SCOPE="routine"
    printf 'Tier: routine — %s property cases per law. Run `tools/verify.sh --total` for the deep slate.\n\n' \
        "$PROPTEST_CASES"

    slate_audit

    # One pass: unit tests, integration binaries and doctests — including the `compile_fail`
    # escape guards, which are doctests. The total tier has to split these, since llvm-cov cannot
    # run doctests; here nothing instruments the build, so there is nothing to split.
    run tests 'tests FAILED' cargo test --workspace --quiet
    ok tests "ok ($(passed) passed, unit + doctests)" 'tests ok'

    release_step
    snippets_step
    cellgraph_surface
    clippy_step --workspace --all-targets
    doclinks_step

    summary
    exit 0
fi

SCOPE="total"
printf 'Tier: total — %s property cases per law, coverage, Miri, module graph.\n\n' "$PROPTEST_CASES"

slate_audit

# `--workspace`, so the reading covers every workspace crate rather than the root one: the embedded
# crates are koan's own code, and a slate that scored only `src/` would let a whole crate ship with
# no coverage signal at all.
run tests 'tests FAILED' cargo llvm-cov --quiet --workspace --lcov --output-path "$LCOV"
ok tests "ok ($(passed) passed → $LCOV)" 'tests ok'

# llvm-cov does not run doctests (instrumented doctests are nightly-only), so the `compile_fail`
# escape guards on the lifetime-erasure accessors go unchecked above. Run them here: a
# `compile_fail` doctest that *starts* compiling is a test failure.
run doctests 'doctests FAILED' cargo test --workspace --doc --quiet
ok doctests "ok ($(passed) passed, compile_fail guards included)" 'doctests ok'

release_step
cellgraph_surface
# `cellgraph/perf` is the one feature in the workspace, and it gates the measurement binary the
# perf step below runs. A default build hides that source from clippy, so the total tier's lint
# turns it on — the same reason the cellgraph-only routine scope does.
clippy_step --workspace --all-targets --features cellgraph/perf
doclinks_step

# The leak/UB audit over the slate the `slate-audit` step just proved current. Minutes, and the
# reason the total tier is not something you run per commit. Under `KOAN_REBASELINE` it records the
# run's duration in observe/miri_slate.md, the way coverage and the score record theirs below.
run miri 'Miri FAILED' python3 tools/miri.py ${REBASELINE:+--log}
ok miri "$(grep -oE '[0-9]+ passed' <<<"$OUT" | head -1) under tree borrows, no leaks" 'miri ok'

run coverage 'coverage FAILED' python3 tools/coverage.py --lcov "$LCOV" \
    ${REBASELINE:+--baseline observe/coverage.txt}
delta="$(compact "$(grep '^coverage: line' <<<"$OUT" | tail -1)")"
ok coverage "line $delta" "coverage $delta"

# What every public cellgraph verb cost, against the newest SHA in cellgraph/observe/perf.csv —
# rebuilt and run beside this sweep, so the two readings share an afternoon. Only the deterministic
# half gates: a verb that allocates more bytes, or more times, than it did at that SHA fails here.
# Time is reported and never gated, because the bar a row would be held to is this machine's own
# spread rather than anything about the change. Under KOAN_REBASELINE the sweep appends this HEAD's
# rows to the record.
run perf 'cellgraph perf: allocations or bytes ROSE' \
    python3 tools/cellgraph_perf.py --gate --quiet ${REBASELINE:+--record}
delta="$(sed -E 's/^cellgraph perf: //' <<<"$(grep -m1 '^cellgraph perf:' <<<"$OUT")")"
ok perf "$delta" "perf $delta"
[ -n "$VERBOSE" ] || detail "$(grep -v '^cellgraph perf:' <<<"$OUT")"

run 'modgraph tests' 'modgraph tooling tests FAILED' python3 tools/modgraph/tests.py
ok 'modgraph tests' "ok ($(awk '/^Ran [0-9]+ test/ {print $2}' <<<"$OUT") passed)" \
    'modgraph tests ok'

# `regen` runs cargo-modules, re-attributes uses edges to the written import surface (re-export
# correction), refreshes observe/doc_graph.dot, then scores. --quiet drops the per-module report and
# the regeneration progress, leaving the bottom-line score and its delta against the trend log.
run modgraph 'modgraph score FAILED' \
    python3 tools/modgraph regen --root koan --edges "$DOT" --quiet \
    ${REBASELINE:+--baseline observe/complexity.txt}
delta="$(compact "$(grep '^baseline: score' <<<"$OUT" | tail -1)")"
ok modgraph "score $delta" "modgraph score $delta"
[ -n "$VERBOSE" ] || detail "$(grep -oE '\(coupling.*\)$' <<<"$OUT")"

summary
