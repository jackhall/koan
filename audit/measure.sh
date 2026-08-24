#!/usr/bin/env bash
# Whole-program allocation and symbol-mint counts for the recorded shapes in `audit/shapes/`.
#
# Builds the binary with `--features alloc-count`, which wraps its allocator in the
# delegating counter (`audit/counting_alloc.rs`) and arms the symbol-mint tally in
# `machine::model::labels`, then runs each shape and reads the `allocations: N` and
# `symbols_minted: N` lines the counted binary writes to stderr. Both numbers are *process*
# totals: interpreter startup, parse, and the run itself, so a shape's signal is the
# margin over an empty program rather than the absolute figure.
#
# Debug profile, matching every other measurement in the repo — a release build inlines
# enough to move the count, and the shapes exist to compare against each other over time,
# not to state a shipped cost.
#
# The counts are transcribed into `audit/README.md` by hand. That table is the committed
# baseline; this script is how it is reproduced.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

cargo build --quiet --features alloc-count

printf '%-26s %-12s %s\n' shape allocations symbols
printf '%-26s %-12s %s\n' -------------------------- ----------- -----------
for shape in audit/shapes/*.koan; do
    # The counts ride stderr beside any region-audit output; the shape's own PRINT goes to
    # stdout and is dropped. `|| true` keeps a shape that errors from aborting the sweep —
    # its missing row is the report.
    report="$(./target/debug/koan "$shape" 2>&1 >/dev/null || true)"
    count="$(printf '%s\n' "$report" | sed -n 's/^allocations: //p')"
    minted="$(printf '%s\n' "$report" | sed -n 's/^symbols_minted: //p')"
    printf '%-26s %-12s %s\n' "$(basename "$shape" .koan)" "${count:-FAILED}" "${minted:-FAILED}"
done
