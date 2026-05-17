# Close the worst test-coverage gaps

**Problem.** [`cargo llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)
(invoked via [`python3 tools/observe_tests.py audit`](../tools/observe_tests.py),
which co-runs the miri-slate audit and writes the lcov report to
[`observe/coverage.lcov`](../observe/coverage.lcov)) reports an overall
91.88% function / 88.75% line coverage across 12,522 lines on the
post-types-refactor branch. The totals look healthy but mask several
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

- *`builtins/fn_def.rs` (67%, with `body.rs` at 65% and
  `argument_bundle.rs` at 67%) — open.* The user-fn definition path is the
  hottest unsafe site on the slate (per-call frame transmute in
  `kfunction/invoke.rs`), and its surrounding builtin is only two-thirds
  covered. Audit the uncovered regions for argument-bundle error paths and
  body-translation edge cases (e.g., empty-body, single-statement-body,
  return-type-deferred body). These are pure-Rust tests, no koan source
  needed.
- *`machine/model/values/kkey.rs` (72%) — open.* Dict-key value type:
  `try_from_kobject` (non-scalar rejection), `Parseable` / `Serializable`
  impls, equality and hashing (including the NaN bit-pattern path on
  `Number`). Test against the three scalar variants plus a non-scalar
  rejection case; the `f64::to_bits()` hashing path needs a NaN-equals-NaN
  check to pin the documented behavior.
- *`machine/execute/nodes.rs` (63%) — open.* Tiny file (~8 executable
  lines per llvm-cov, mostly enum constructor coverage on `NodeOutput` /
  `NodeStep` / `NodeWork`). Coverage is driven by the execute layer's
  integration tests hitting every variant; the gap is one or two unhit
  arms. Identify the missing arms and add a focused execute-path test
  per arm.

## Dependencies

**Requires:** none — every direction is additive test work against shipped code.

**Unblocks:** none directly. Higher-coverage substrate makes the
[module-system stage 5–7](../ROADMAP.md#module-system) refactors safer,
since the typed-API surface they edit (`signature.rs`, `kfunction/*`,
`lift.rs`) currently has the widest gaps.
