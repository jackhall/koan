# Close the worst test-coverage gaps

**Problem.** [`cargo llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)
(invoked via [`python3 tools/observe_tests.py audit`](../tools/observe_tests.py),
which co-runs the miri-slate audit and writes the lcov report to
[`observe/coverage.lcov`](../observe/coverage.lcov)) reports overall
93.17% function / 89.97% line coverage across the post-types-refactor
branch. The totals look healthy but mask several files where coverage
is well below the suite average, leaving regression risk concentrated
in code paths the suite barely exercises.

**Impact.**

- *Regression risk drops where the gap is widest.* Each gap file below
  hosts code the post-refactor branch reorganized; without targeted
  coverage, a future refactor can land structurally clean (modgraph green,
  miri-slate clean) while breaking behavior the gap file owns.
- *Coverage trend becomes a real signal.* With the worst gaps closed,
  per-PR coverage drift in `observe/coverage.lcov` becomes a meaningful
  review check rather than noise dominated by a handful of always-low files.

**Directions.**

- *`machine/core/kfunction/body.rs` (65%) and
  `machine/core/kfunction/argument_bundle.rs` (67%) — open.* Sibling files
  to `fn_def.rs` (now at 88% via
  [`src/builtins/fn_def/tests/body_routing.rs`](../src/builtins/fn_def/tests/body_routing.rs)).
  `body.rs` hosts the `Body` enum + `KFunction::invoke` per-call frame
  transmute — the hottest unsafe site on the miri slate; `argument_bundle.rs`
  hosts the slot-extraction helpers. Audit the uncovered regions for the
  argument-bundle error paths and the invoke-time frame-lift edge cases the
  `fn_def.rs` tests don't reach.
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
