#!/usr/bin/env bash
# Run the koan build-verification slate: instrumented unit tests (cargo
# llvm-cov), doctests (including `compile_fail` guards, which llvm-cov does not
# run), lints, doclinks, tutorial-snippet output checks, and the modgraph
# fractal-complexity score.
# Mirrors the `verify` skill (.claude/skills/verify/).
#
# The modgraph, coverage, and allocation-audit steps print current readings. They
# rebaseline `observe/complexity.txt` / `observe/coverage.txt` / `observe/alloc/` only when invoked with
# `KOAN_REBASELINE` set — pre-commit sets it; manual runs leave it unset,
# since the trend logs should record one entry per commit, not one per
# local sanity-check.
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

step() { printf '\n=== %s ===\n' "$*"; }

# Run a command, hiding its (voluminous, all-green) output. On failure, replay
# the captured output and propagate the exit status so `set -e` aborts the slate.
# Used for the steps whose only green output is test-runner chatter (progress
# dots, per-binary "test result: ok" lines) with no summary worth keeping.
quiet() {
    local log status
    log="$(mktemp)"
    if "$@" >"$log" 2>&1; then
        rm -f "$log"
    else
        status=$?
        cat "$log"
        rm -f "$log"
        return "$status"
    fi
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
    printf '\nChange scope: workgraph only — running the library slate.\n'

    # Runs unit tests and doctests in one pass — unlike the full slate, where the
    # doctests need their own step because llvm-cov cannot run them. That covers the
    # `compile_fail` escape guards, which are doctests.
    #
    # `test-hooks` widens the white-box surface koan's own tests reach. It is off in
    # a default build, so compiling it here is what keeps the gated code in the slate.
    step "1/3 cargo test -p workgraph (unit tests + doctests, --features test-hooks)"
    quiet cargo test -p workgraph --features test-hooks --quiet

    step "2/3 cargo clippy -p workgraph"
    if ! out="$(cargo clippy -p workgraph --all-targets --features test-hooks -- -D warnings 2>&1)"; then
        printf '%s\n' "$out"
        cargo clippy -p workgraph --fix --allow-dirty --allow-staged --all-targets --features test-hooks
        cargo clippy -p workgraph --all-targets --features test-hooks -- -D warnings
    fi

    step "3/3 doclinks check"
    python3 tools/doclinks.py check --gates-only

    # Informational, never gating: koan failing to compile against workgraph HEAD is
    # the expected mid-migration state, and its size is the adoption debt now owed.
    step "koan adoption status (informational)"
    if koan_out="$(cargo check -p koan --all-targets 2>&1)"; then
        echo "koan compiles against workgraph HEAD — no adoption debt."
    else
        echo "koan does NOT compile against workgraph HEAD:" \
             "$(printf '%s\n' "$koan_out" | grep -c '^error')" "errors." \
             "Adoption is owed by a koan-side item; see \`cargo check -p koan --all-targets\`."
    fi

    exit 0
fi

step "1/9 cargo llvm-cov (instrumented tests → $LCOV)"
quiet cargo llvm-cov --quiet --lcov --output-path "$LCOV"

# llvm-cov does not run doctests (instrumented doctests are nightly-only), so the
# `compile_fail` escape guards on the lifetime-erasure accessors go unchecked above.
# Run them here: a `compile_fail` doctest that *starts* compiling is a test failure.
step "2/9 cargo test --doc (doctests + compile_fail guards)"
quiet cargo test --doc --quiet

step "3/9 cargo clippy"
if ! out="$(cargo clippy --all-targets -- -D warnings 2>&1)"; then
    printf '%s\n' "$out"
    cargo clippy --fix --allow-dirty --allow-staged --all-targets
    cargo clippy --all-targets -- -D warnings
fi

step "4/9 doclinks check"
# --gates-only drops the informational source-tree changes report; the four
# gating audits (links, deps, orphans, next-items) still run and still gate.
python3 tools/doclinks.py check --gates-only

# The tutorial's runnable snippets (```koan blocks with an expected ```text output)
# are diffed against the interpreter. Needs the plain debug binary — llvm-cov above
# builds an instrumented one under a different profile, so build it explicitly.
step "5/9 tutorial snippets"
quiet cargo build --quiet
python3 tools/verify_snippets.py

# The allocation shapes in `audit/shapes/`, swept through the counting allocator
# (`--features alloc-count`) and the bounded regression test's brackets. Reports each
# shape's totals, the marginal terms differenced out of them, and the headroom left on
# every bound in `tests/allocation_baseline.rs`. Never gating: the bounds themselves
# are asserted by that test, which step 1 ran.
step "6/9 allocation audit (record: observe/alloc/)"
python3 tools/alloc_audit.py ${REBASELINE:+--baseline}

step "7/9 coverage delta (lcov: $LCOV)"
python3 tools/coverage.py --lcov "$LCOV" \
    ${REBASELINE:+--baseline observe/coverage.txt}

step "8/9 modgraph tooling tests"
quiet python3 tools/modgraph/tests.py

step "9/9 modgraph score (DOT: $DOT)"
# `regen` runs cargo-modules, re-attributes uses edges to the written import
# surface (re-export correction), refreshes observe/doc_graph.dot, then scores.
# --quiet drops the per-module report, leaving the bottom-line score (and delta).
python3 tools/modgraph regen --root koan --edges "$DOT" --quiet \
    ${REBASELINE:+--baseline observe/complexity.txt}
