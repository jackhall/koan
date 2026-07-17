#!/usr/bin/env bash
# Run the koan build-verification slate: instrumented unit tests (cargo
# llvm-cov), doctests (including `compile_fail` guards, which llvm-cov does not
# run), lints, doclinks, tutorial-snippet output checks, and the modgraph
# fractal-complexity score.
# Mirrors the `verify` skill (.claude/skills/verify/).
#
# The modgraph and coverage steps print current scores. They rebaseline
# `observe/complexity.txt` / `observe/coverage.txt` only when invoked with
# `KOAN_REBASELINE` set — pre-commit sets it; manual runs leave it unset,
# since the trend logs should record one entry per commit, not one per
# local sanity-check.
#
# Outputs (override paths via env vars):
#   - DOT graph from cargo-modules → observe/modules.dot   (`KOAN_DOT`)
#   - llvm-cov lcov report          → observe/coverage.lcov (`KOAN_LCOV`)

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

DOT="${KOAN_DOT:-observe/modules.dot}"
LCOV="${KOAN_LCOV:-observe/coverage.lcov}"
REBASELINE="${KOAN_REBASELINE:-}"

step() { printf '\n=== %s ===\n' "$*"; }

step "1/8 cargo llvm-cov (instrumented tests → $LCOV)"
cargo llvm-cov --quiet --lcov --output-path "$LCOV"

# llvm-cov does not run doctests (instrumented doctests are nightly-only), so the
# `compile_fail` escape guards on the lifetime-erasure accessors go unchecked above.
# Run them here: a `compile_fail` doctest that *starts* compiling is a test failure.
step "2/8 cargo test --doc (doctests + compile_fail guards)"
cargo test --doc --quiet

step "3/8 cargo clippy"
if ! cargo clippy --all-targets -- -D warnings; then
    cargo clippy --fix --allow-dirty --allow-staged --all-targets
    cargo clippy --all-targets -- -D warnings
fi

step "4/8 doclinks check"
python3 tools/doclinks.py check

# The tutorial's runnable snippets (```koan blocks with an expected ```text output)
# are diffed against the interpreter. Needs the plain debug binary — llvm-cov above
# builds an instrumented one under a different profile, so build it explicitly.
step "5/8 tutorial snippets"
cargo build --quiet
python3 tools/verify_snippets.py

step "6/8 coverage delta (lcov: $LCOV)"
python3 tools/coverage.py --lcov "$LCOV" \
    ${REBASELINE:+--baseline observe/coverage.txt}

step "7/8 modgraph tooling tests"
python3 tools/modgraph/tests.py

step "8/8 modgraph score (DOT: $DOT)"
# `regen` runs cargo-modules, re-attributes uses edges to the written import
# surface (re-export correction), refreshes observe/doc_graph.dot, then scores.
python3 tools/modgraph regen --root koan --edges "$DOT" \
    ${REBASELINE:+--baseline observe/complexity.txt}
