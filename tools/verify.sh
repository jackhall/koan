#!/usr/bin/env bash
# Run the koan build-verification slate: instrumented unit tests (cargo
# llvm-cov), lints, doclinks, and the modgraph fractal-complexity score.
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

step "1/5 cargo llvm-cov (instrumented tests → $LCOV)"
cargo llvm-cov --quiet --lcov --output-path "$LCOV"

step "2/5 cargo clippy"
if ! cargo clippy --all-targets -- -D warnings; then
    cargo clippy --fix --allow-dirty --allow-staged --all-targets
    cargo clippy --all-targets -- -D warnings
fi

step "3/5 doclinks check"
python3 tools/doclinks.py check

step "4/5 coverage delta (lcov: $LCOV)"
python3 tools/coverage.py --lcov "$LCOV" \
    ${REBASELINE:+--baseline observe/coverage.txt}

step "5/5 modgraph score (DOT: $DOT)"
cargo modules dependencies --package koan --lib \
    --no-externs --no-sysroot --no-traits --no-fns --no-types \
    > "$DOT"
python3 tools/modgraph.py --edges "$DOT" --root koan \
    ${REBASELINE:+--baseline observe/complexity.txt}
