# Dependent parameter annotations

**Problem.** A parameter's declared type can't reference an earlier
parameter in the same FN signature. Writing
`(FN (MAKE T: Type elt: T) -> T = ...)` errors at FN-definition because
`T` isn't bound at signature-elaboration time — parameter types resolve
against the FN's outer scope, not against per-call parameter values.

OCaml's multi-parameter functor signatures carry this exact shape:
`module Make (E : ORDERED) (S : SET with type elt = E.t) = ...` — the
second parameter's signature mentions the first. Without dependent
annotations, multi-parameter functors can't express cross-parameter
sharing constraints at the parameter list; the workaround is encoding
the constraint in the body or routing through a paired tuple-module.

**Impact.**

- *Multi-parameter OCaml-style functors with sharing constraints become
  writable.* Generalizes the single-parameter functor surface
  described in
  [design/module-system.md § Functors](../design/module-system.md#functors)
  so the second parameter's signature can pin a slot to the first
  parameter's abstract type.
- *Dependent value-typed parameters become writable.* Constructions
  like `(BUILD T: Type x: T)` — accept a type, then accept a value of
  that type — are first-class.

**Directions.**

- *Carrier — decided.* Reuse the
  `ReturnType { Resolved(KType), Deferred(DeferredReturn) }` carrier
  shipped at
  [`ExpressionSignature::return_type`](../src/runtime/machine/model/types/signature.rs).
  Parameter type slots widen to the same two-variant shape; selection
  is decidable at FN-definition by scanning each parameter type's
  `TypeExpr` for any leaf matching an *earlier* parameter name.

- *Dispatch staging — open.* The hard problem. Today the dispatcher
  resolves admissibility against a holistic dispatch index — every
  slot's type is concrete at definition. A `Deferred` parameter type
  means slot N's admissibility depends on the value bound to slot
  M < N. Two paths:
  - (a) *Staged left-to-right dispatch.* At dispatch time, resolve
    parameters in order. After binding slot M, install M into a
    per-dispatch scope; re-elaborate slot N's `Deferred` type against
    that scope; admissibility-check slot N. Touches
    `KFunction::accepts_for_wrap`, `Scope::resolve_dispatch`, and the
    dispatch index's lookup keys.
  - (b) *Index-side projection.* Compute admissibility partially at
    definition (against everything that *can* be resolved) and
    complete the check at dispatch time. Lighter on dispatch, heavier
    on the index.

- *Overload conflict rules — open.* Two FNs with the same fixed-token
  shape but different dependent-annotation patterns
  (one `(MAKE T: Type elt: T)`, the other `(MAKE T: Type elt: Number)`)
  need a comparison rule for "more specific." Today's overload
  resolution is concrete-type-keyed; dependent annotations need a
  partial-order extension.

## Dependencies

None — the SIG-side surface for declaring slots whose type is an
earlier-parameter reference (the `elt: T` shape in `(BUILD T: Type elt:
T)`) shares its surface form with the SIG-body `VAL` declarator already
shipped at
[design/module-system.md § Structures and signatures](../design/module-system.md#structures-and-signatures).
