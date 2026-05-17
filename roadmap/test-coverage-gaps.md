# Close the worst test-coverage gaps

**Problem.** [`cargo llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)
(invoked via [`python3 tools/observe_tests.py audit`](../tools/observe_tests.py),
which co-runs the miri-slate audit and writes the lcov report to
[`observe/coverage.lcov`](../observe/coverage.lcov)) reports a single
remaining file well below the suite average, leaving regression risk
concentrated in code paths the suite barely exercises.

**Impact.**

- *Regression risk drops where the gap is widest.* The remaining gap file
  hosts code the post-refactor branch reorganized; without targeted
  coverage, a future refactor can land structurally clean (modgraph green,
  miri-slate clean) while breaking behavior the gap file owns.
- *Coverage trend becomes a real signal.* With the last gap closed,
  per-PR coverage drift in `observe/coverage.lcov` becomes a meaningful
  review check rather than noise dominated by an always-low file.

**Directions.**

- *`machine/model/values/kkey.rs` (72%) — open.* Dict-key value type:
  `try_from_kobject` (non-scalar rejection), `Parseable` / `Serializable`
  impls, equality and hashing (including the NaN bit-pattern path on
  `Number`). Test against the three scalar variants plus a non-scalar
  rejection case; the `f64::to_bits()` hashing path needs a NaN-equals-NaN
  check to pin the documented behavior.

## Dependencies

**Requires:** none — every direction is additive test work against shipped code.

**Unblocks:** none directly. Higher-coverage substrate makes the
[module-system stage 5–7](../ROADMAP.md#module-system) refactors safer,
since the typed-API surface they edit (`signature.rs`, `kfunction/*`,
`lift.rs`) currently has the widest gaps.
