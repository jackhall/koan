# Close the worst test-coverage gaps

**Problem.** [`cargo llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)
(invoked via [`python3 tools/observe_tests.py audit`](../tools/observe_tests.py),
which co-runs the miri-slate audit and writes the lcov report to
[`observe/coverage.lcov`](../observe/coverage.lcov)) reports an overall
88.21% region / 90.71% function / 87.15% line coverage across 11,876 lines
on the post-types-refactor branch. The totals look healthy but mask several
files where coverage is well below the suite average, leaving regression
risk concentrated in code paths the suite barely exercises.

**Impact.**

- *Regression risk drops where the gap is widest.* Each gap file below
  hosts code the post-refactor branch reorganized; without targeted
  coverage, a future refactor can land structurally clean (modgraph green,
  miri-slate clean) while breaking behavior the gap file owns.
- *Coverage trend becomes a real signal.* With the worst gaps closed,
  per-PR coverage drift in `observe/coverage.lcov` becomes a meaningful
  review check rather than noise dominated by a handful of always-low files.

**Directions.**

- *`machine/model/types/signature.rs` (64%) — open.* Signature shape
  predicates — `is_*`, equality, the `Display` impl. Pair with the
  module-system stage-2-and-beyond signature work; add tests for each
  predicate against the boundary shapes (empty, single-element, all-pin,
  mixed VAL/SIG, deferred-return).
- *`builtins/fn_def.rs` (67%, with `body.rs` at 65% and
  `argument_bundle.rs` at 67%) — open.* The user-fn definition path is the
  hottest unsafe site on the slate (per-call frame transmute in
  `kfunction/invoke.rs`), and its surrounding builtin is only two-thirds
  covered. Audit the uncovered regions for argument-bundle error paths and
  body-translation edge cases (e.g., empty-body, single-statement-body,
  return-type-deferred body). These are pure-Rust tests, no koan source
  needed.
- *`parse/frame.rs` (66%) — open.* Frame-builder edge cases — likely the
  error branches the happy-path parse tests don't hit. Read the frame
  state-machine, list its rejection conditions, and write one parse-error
  test per condition. Cheap.
- *`machine/core/pending.rs` (69%) — open.* Forward-reference resolution
  state. `tests/forward_reference_resolves.rs` covers the happy path; what's
  missing are the failure / timeout / multi-name cycle paths. Add tests
  alongside the existing integration suite so the forward-resolution surface
  has one file that pins both success and failure shapes.

## Dependencies

**Requires:** none — every direction is additive test work against shipped code.

**Unblocks:** none directly. Higher-coverage substrate makes the
[module-system stage 5–7](../ROADMAP.md#module-system) refactors safer,
since the typed-API surface they edit (`signature.rs`, `kfunction/*`,
`lift.rs`) currently has the widest gaps.
