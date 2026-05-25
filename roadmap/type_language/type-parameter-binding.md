# Per-call type-parameter binding in parameter signatures

Free type-parameter names in an FN's parameter signatures get bound per
call. The binding source is one of two: extraction from an argument's
carried type structure, or the value of an earlier parameter in the same
signature. Both sub-cases share the same `Deferred`-carrier widening, the
same per-call scope installation seam, and the same deferred-return
re-elaboration path.

**Problem.** A parameter's declared type can't reference a free name
today — neither one that's *inferred* from an argument's type structure
nor one *bound by* an earlier parameter in the same signature. Parameter
types resolve against the FN's outer scope at definition time, so any
free name has nowhere to bind:

- *Type-parameter inference from value-slot arguments.* Parameterized
  values carry their type arguments
  ([ktype.md § Runtime type-parameter carriers](../../design/typing/ktype.md#runtime-type-parameter-carriers)),
  and a `:(List T)` / `:(Result T E)` slot admits a value structurally
  via the [`matches_value`](../../src/machine/model/types/ktype_predicates.rs)
  `ConstructorApply` / container arms. But a free type-parameter name in
  the slot has nowhere to bind: `FN head (xs :(List T)) -> :T = ...`
  can't be defined, because `T` in the deferred return resolves through
  scope and nothing populates that scope from the argument the call
  actually carried. The generic-destructuring unifier
  ([`unify_slot`](../../src/machine/model/types/unify.rs)) — which walks
  a slot's surface `TypeExpr` against a value's carried `KType` and
  collects `(param_name, concrete)` bindings — is unit-tested but not
  wired into [`invoke.rs`](../../src/machine/core/kfunction/invoke.rs);
  the re-export is `#[allow(unused_imports)]` at the crate level.

- *Cross-parameter references in the same signature.* `(FN (MAKE T:
  Type elt: T) -> T = ...)` errors at FN-definition because `T` isn't
  bound at signature-elaboration time. OCaml's multi-parameter functor
  signatures carry this exact shape — `module Make (E : ORDERED) (S :
  SET with type elt = E.t) = ...` — the second parameter's signature
  mentions the first. Without it, multi-parameter functors can't express
  cross-parameter sharing constraints at the parameter list; the
  workaround is encoding the constraint in the body or routing through
  a paired tuple-module.

Both forms hit the same wall: the parameter type slot is a `KType`
resolved once against the FN's outer scope. There's no carrier for "this
slot's type is determined per call against the per-dispatch frame."

**Impact.**

- *Generic value-slot functions become definable.* `FN head (xs :(List
  T)) -> :T` and `FN unwrap (r :(Result T E)) -> :T` type-check per call
  against the argument the caller actually passed. Type-parameter
  dispatch on value slots resolves the deferred return against the
  bound concrete type, so the per-call return matches the argument's
  instantiation.
- *Multi-parameter OCaml-style functors with sharing constraints become
  writable.* Generalizes the single-parameter functor surface
  ([design/typing/functors.md](../../design/typing/functors.md)) so the
  second parameter's signature can pin a slot to the first parameter's
  abstract type.
- *Dependent value-typed parameters become writable.* Constructions like
  `(BUILD T: Type x: T)` — accept a type, then accept a value of that
  type — are first-class.
- *One per-call scope-installation seam serves both sources.* Both
  binding paths (argument-extracted, earlier-parameter-bound) install
  `(param_name, concrete)` into the per-call scope via
  `Scope::register_type` before the deferred return elaborates — same
  call site, same scope-write semantics, same downstream elaboration.

**Directions.**

- *Carrier widening — decided.* Parameter type slots widen to the same
  `ReturnType { Resolved(KType), Deferred(DeferredReturn) }` carrier
  shipped at
  [`ExpressionSignature::return_type`](../../src/machine/model/types/signature.rs).
  Selection at FN-definition: scan each parameter type's `TypeExpr` for
  leaves matching either an earlier parameter name *or* a slot-local
  free type-parameter name (e.g. `T` in `:(List T)`).
- *Binding-source unification — decided.* Both sources install
  `(param_name, concrete)` into the per-call scope via
  `Scope::register_type` before deferred elaboration runs. `unify_slot`
  handles the argument-extraction case; the earlier-parameter case
  routes through the existing dual-write path for type-denoting
  parameter values.
- *Per-call binding site — decided.* The invoke dual-write site, before
  deferred-return elaboration — the seam `unify_slot` was built against.
- *Type-parameter representation — decided.* No `KType::TypeParam`
  variant; type parameters stay ordinary scope-resolved names, and the
  unifier identifies a leaf as a parameter by membership in the
  caller-supplied `params` set.
- *Dispatch staging — open.* Slot N's admissibility depends on bindings
  from slot M < N. Two paths:
  - *(a) Staged left-to-right dispatch.* At dispatch time, resolve
    parameters in order. After binding slot M, install M into a
    per-dispatch scope; re-elaborate slot N's `Deferred` type against
    that scope; admissibility-check slot N. Touches
    `KFunction::accepts_for_wrap`, `Scope::resolve_dispatch`, and the
    dispatch index's lookup keys.
  - *(b) Index-side projection.* Compute admissibility partially at
    definition (against everything that *can* be resolved) and complete
    the check at dispatch time. Lighter on dispatch, heavier on the
    index.
- *Overload conflict rules — open.* Two FNs with the same fixed-token
  shape but different dependent-annotation patterns (one `(MAKE T:
  Type elt: T)`, the other `(MAKE T: Type elt: Number)`) need a
  comparison rule for "more specific." Today's overload resolution is
  concrete-type-keyed; dependent annotations need a partial-order
  extension.
- *Tripwire extension — decided.* The existing
  [`function_compat`](../../src/machine/model/types/ktype_predicates.rs)
  `debug_assert!` that guards deferred-return Any-coarsening
  extends over argument-slot deferreds — same invariant ("no consumer
  compares a deferred-carrying slot against a non-`Any` structural slot
  yet"), same forcing condition. The full precision fix stays parked in
  [kfunction-deferred-ret-precision.md](kfunction-deferred-ret-precision.md),
  whose scope grows to cover *parameter and return* slots in one pass
  once the tripwire fires.

## Dependencies

**Requires:**

None — the substrate (runtime carriers, `matches_value` admission arms,
ascription stamping, `unify_slot` core, `ReturnType` carrier, dual-write
for type-denoting parameters) is shipped; this item wires the existing
machinery into the invoke path and widens parameter slots to admit the
deferred-carrier shape.

**Unblocks:**

- [Structural KFunction admission across deferred parameter and return slots](kfunction-deferred-ret-precision.md)
  — widening parameter slots to the `Deferred(_)` carrier and extending
  the tripwire over them grows the precision item's scope to cover both
  surfaces in one pass.

`List` / `Dict` element-type dispatch and the builtin `Result`
parameterized type
([error-handling](../../design/error-handling.md)) already function on
the shipped carriers; this item extends them to generic value-slot
signatures and to cross-parameter-referencing signatures.
