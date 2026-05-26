# FUNCTOR return-validation fold

Collapse the two-phase classify-then-validate path in
[`fn_def/return_type.rs`](../../src/builtins/fn_def/return_type.rs) into a
single walk that emits both Resolved/Deferred classification and the
FUNCTOR denotation verdict in one pass.

**Problem.** The FUNCTOR binder ships with classification and validation
as separate passes: `classify_return_type` runs the parameter-name scan
to choose Resolved/Deferred, then a second walk inspects the same
carrier to decide whether its head denotes a module, signature, or
functor. Both passes traverse the same surface form. Two consequences in
the runtime:

- *Redundant traversal of the return-type carrier.* Every FUNCTOR
  definition walks the carrier twice — once for parameter-name search,
  once for head-shape validation. Cheap per-call, but the two passes
  duplicate the shape dispatch on `SIG_WITH` / `MODULE_TYPE_OF` /
  bare-ident / `(Functor …)`.
- *Two seams to keep in sync.* Adding a new admissible carrier (e.g.
  another type-position sigil) requires touching both the classifier
  and the validator. The two are independent today but read the same
  surface form, so drift between them surfaces as silently-misclassified
  carriers (admissible but Deferred, or inadmissible but Resolved).

**Impact.**

- *One traversal of the return-type carrier per FUNCTOR definition.*
  Classification and validation share a walk; new admissible carriers
  extend a single match.
- *Validation and classification stop drifting.* The fused walk reports
  `(ReturnTypeState, AdmissibleVerdict)` together, so a new carrier
  shape is admitted (or rejected) by both at once.

**Directions.**

- *Walk shape — open.* Two natural shapes: (a) extend
  `classify_return_type` to return a tuple `(ReturnTypeState,
  AdmissibleVerdict)`; (b) replace the two functions with a single
  `analyze_return_carrier` that emits a richer enum carrying both axes.
  Recommended: (a) — preserves the existing call sites for non-FUNCTOR
  paths (FN doesn't consult the verdict) and lets the FUNCTOR builder
  destructure the tuple.
- *Verdict carrier for Deferred arms — open.* The Deferred-arm verdict
  for a bare-parameter ref depends on the parameter's declared type
  (admissible iff type-denoting). The single walk either takes the
  parameter list as input (so the verdict is final after one pass) or
  emits a "needs-parameter-lookup" verdict the FUNCTOR builder resolves
  against the already-collected parameter list. Recommended: take the
  parameter list as input — by the time `classify_return_type` runs in
  fn_def, the param list is already in hand.

## Dependencies

**Requires:**

