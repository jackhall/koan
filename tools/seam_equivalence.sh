#!/usr/bin/env bash
# The record-escape-seam equivalence battery.
#
# The `seam-force-copy` and `seam-force-pin` cargo features (mutually exclusive)
# force every record escape seam to a single verb — copy or pin. The whole
# output-asserting test suite is the equivalence battery: identical hardcoded
# expectations passing under BOTH forced verbs prove the cost-driven copy-vs-pin
# choice is semantically invisible (it changes only which memory mechanism runs,
# never language output). Three mechanism-census tests legitimately assert the
# internal verb and are cfg-gated out of the build that overrides them; the
# language-output tests are never gated.
#
# This runs on demand / in CI, NOT in the per-commit hook: each forced build
# recompiles the whole crate under a distinct cfg (a separate incremental cache),
# so folding two extra full `cargo test` cycles into every commit would bloat the
# already ~3-6 min hook for a property that changes only when the seam-policy code
# changes. The default build is covered by `tools/verify.sh`; it is re-run here so
# the battery is self-contained.
#
# Usage: tools/seam_equivalence.sh

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

step() { printf '\n=== %s ===\n' "$*"; }

step "1/3 default build (cost-driven chooser)"
cargo test

step "2/3 --features seam-force-copy (every record escape copies)"
cargo test --features seam-force-copy

step "3/3 --features seam-force-pin (every record escape pins)"
cargo test --features seam-force-pin

printf '\nequivalence battery green: language output is identical under copy and pin.\n'
