# Sparse-carrier correlation in parameterized-type constructors

**Problem.** The parameterized-type constructors (`body_list_of`, `body_map`,
`body_apply_as` in
[parameterized_types.rs](../../src/builtins/parameterized_types.rs)) build a
composite `KType` (`List` / `Dict` / `ConstructorApply`) embedding their
args, and store it through `alloc_type_with(carriers, kt)` with `carriers`
assembled by flattening per-arg `arg_carrier` lookups. `arg_carrier` is
sparse — a region-free arg contributes no carrier — so the deps list does not
positionally correspond to the embedded args: with `k` a scalar, `views[0]`
is `v`'s view. The sites sidestep the mismatch by capturing the ambient arg
values and discarding the views entirely — the capture shape [fold-closure
capture provenance](fold-closure-provenance.md) names — so a views-based
rebuild at the fold brand has no way to tell which view is which arg.

**Acceptance criteria.**

- Each parameterized-type constructor builds its composite `KType` at the
  store's fold brand from operands that identify every embedded arg (by
  totality or by key), regardless of which args carried reach; no
  ambient-lifetime `KType` is captured into a folded placement at these
  sites.
- A region-free arg still seals without folding foreign reach — the scalar
  gate's exact-reach behavior is preserved.
- The full test suite and the Miri audit slate are green across the change.

**Directions.**

- *Correlation mechanism — open.* (a) Total operand set — every embedded arg
  crosses the brand as an operand, region-free args as owned/`'static`
  operands; (b) keyed views — the fold surface hands a name-correlated view
  bundle instead of a positional `Vec`; (c) per-arg fold chain with explicit
  position bookkeeping (weakest — re-encodes the correlation by hand).
  Recommended: (a) when the operand cost is acceptable, else (b).

## Dependencies

**Requires:** none — operates on the current fold surface.

**Unblocks:**

- [Fold-closure capture provenance](fold-closure-provenance.md) — the
  constructor sites must correlate views to args before their captures can
  move inside the brand.
