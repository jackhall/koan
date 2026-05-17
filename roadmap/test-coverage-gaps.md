# Close the worst test-coverage gaps

**Problem.** [`cargo llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)
(invoked via [`python3 tools/observe_tests.py audit`](../tools/observe_tests.py),
which co-runs the miri-slate audit and writes the lcov report to
[`observe/coverage.lcov`](../observe/coverage.lcov)) reports an overall
87.03% region / 89.62% function / 86.06% line coverage across 11,641 lines
on the post-types-refactor branch. The totals look healthy but mask several
files where coverage is well below the suite average — including one
production builtin (`FUNCTION_OF`) that has zero test exercise — meaning the
slate is silently signing off on code paths nothing actually drives.

**Impact.**

- *Regression risk drops where the gap is widest.* Each gap file below
  hosts code the post-refactor branch reorganized; without targeted
  coverage, a future refactor can land structurally clean (modgraph green,
  miri-slate clean) while breaking behavior the gap file owns.
- *Coverage trend becomes a real signal.* With the worst gaps closed,
  per-PR coverage drift in `observe/coverage.lcov` becomes a meaningful
  review check rather than noise dominated by a handful of always-zero files.
- *Dead vs. unfinished code surfaces.* The 0% files force a deliberate
  decision — write the test, or delete the code — instead of leaving the
  symbol in the public surface as ambiguous dead weight.

**Directions.**

- *`builtins/type_ops/function_of.rs` (0% — entire file, 33 lines, 1 fn) —
  open.* The `FUNCTION_OF` builtin compiles and binds but no test drives
  it. Two alternatives: write the minimal-shape test that pins down its
  result type (mirroring `LIST_OF` / `DICT_OF` siblings, which are 80%+
  covered), or remove the binding if its role was subsumed by a sibling.
  Recommended: write the test first — `function_of.rs` has the same shape
  as its 80%+-covered siblings, so the unused-binding hypothesis is the
  less-likely failure mode, and the test is the smaller artifact.
- *`machine/core/kerror.rs` (31%) — open.* Error-display / `Display`
  formatting paths are the obvious untested surface. Add a `Display` round-
  trip test per `KErrorKind` variant — most variants already have a
  `kind == ...` assertion at the construction site, so the missing piece is
  the rendered-output assertion that pins format strings against accidental
  rewording.
- *`machine/execute/lift.rs` (57%) — open.* Half the lift policy is uncovered
  despite being on the per-call-arena reclamation path the miri slate pins
  (`unanchored_kfuture_*` tests anchor the policy decision but not its
  branches). Audit the uncovered regions against the lift-decision table in
  [design/memory-model.md](../design/memory-model.md) and add behavior tests
  for the branches that have no coverage line — likely the lift-recursive
  composite-value walk and the anchored-with-borrow handoff.
- *`machine/model/ast.rs` (63%) — open.* AST helpers (`KExpression`
  pretty-print, the `ExpressionPart` accessors, `KLiteral` round-trips). The
  parse-side tests exercise construction but not the readback helpers used
  by error messages and dispatch. Pair with the `kerror.rs` work — the AST
  helpers most-likely uncovered are the ones invoked by error rendering.
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
