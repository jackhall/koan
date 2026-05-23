# FUNCTOR binder

Bring the runtime in line with [design/typing/functors.md](../../design/typing/functors.md):
a dedicated `FUNCTOR` binder with definition-time return-type validation, a
distinct `KType::KFunctor` variant with a one-way admissibility wall
against `KType::KFunction`, and a `:(Functor (params) -> R)` type-position
sigil.

**Problem.** Functors are not a first-class binder. An FN whose return
slot happens to denote a module or signature is the only way to write
one, so functor-ness must be inferred from the resolved (or deferred)
return-type carrier at every consumer. Two consequences fall out:

- [`function_value_ktype`](../../src/machine/model/values/kobject.rs)
  and [`function_compat`](../../src/machine/model/types/ktype_predicates.rs)
  coarsen `Deferred(_)` returns to `KType::Any` — the structural
  function-type language has no surface for "this FN is a functor and its
  return slot is module-kind," tracked under
  [kfunction-deferred-ret-precision.md](kfunction-deferred-ret-precision.md).
- No FN-def-time check forces the return slot of an intended functor to
  denote a module or signature; mistakes surface as opaque dispatch-time
  errors several frames removed from the binder.

There is also no type-position surface for "this slot is a functor of this
shape" — only the binder keyword exists, so functor-returning functors
have no admissible return-type denotation.

**Impact.**

- *FUNCTOR return slots are statically validated at the binder.* The
  admissible carriers (`Signature`, `SignatureBound`, `(SIG_WITH …)`,
  `AnyUserType { kind: Module }`, recursively `KType::KFunctor`) are
  checked when the FUNCTOR binder runs; any other denotation surfaces
  `FUNCTOR return-type slot must denote a module, signature, or functor`
  at the FUNCTOR site, not several Dispatch frames downstream.
- *Functors and ordinary functions are type-disjoint.* `KType::KFunctor`
  admits only into other `KType::KFunctor` slots; the
  [`function_compat`](../../src/machine/model/types/ktype_predicates.rs)
  cross-arms refuse both directions of the `KFunctor`/`KFunction` wall, so
  an FN that incidentally returns a module value cannot be passed where a
  functor is expected and vice versa.
- *Functor-returning functors get a type-position denotation.* The
  `:(Functor (params) -> R)` sigil parses and elaborates against
  [`elaborate_type_expr`](../../src/machine/model/types/resolver.rs) as the
  structural functor type, surface-disjoint from the `FUNCTOR` binder
  keyword on the same rule that keeps `FN` and `Function` disjoint.
- *FUNCTOR is the declared seam for future applicative-mode work.*
  Generative-only semantics ship now; once predicate typing lands, the
  applicative opt-in (parked in
  [standard-library.md](../libraries/standard-library.md)) attaches to the
  `is_functor` flag rather than re-deriving functor-ness from the return
  type at every consumer.

**Directions.**

- *Binder design — decided per [design/typing/functors.md](../../design/typing/functors.md).*
  FUNCTOR is the only path to functor semantics (option 1a from the
  design); FN-returning-a-module is just a function returning a value.
- *Underlying value representation — decided.* Same `KObject::KFunction`
  variant and `KFunctionValue` internals, distinguished by an `is_functor:
  bool` field set at binder construction. Dispatch, scheduler, per-call
  scope, and `KFunction::invoke` are unchanged.
- *Surface `KType` — decided.* `function_value_ktype` returns
  `KType::KFunctor { params, ret }` when the flag is set,
  `KType::KFunction { args, ret }` otherwise. The variants share no
  admissibility (one-way wall in `function_compat`).
- *Parameter constraints — decided.* No requirement that any FUNCTOR
  parameter be signature-typed; value-only functors (`FUNCTOR (MAKETREE
  factor :Number) -> …`) are admissible, matching the unified
  value/module-language framing in
  [design/typing/functors.md § Parameters](../../design/typing/functors.md#parameters).
- *Body shape — decided.* Existing `MODULE Result = (...)` form, no new
  anonymous-structure syntax.
- *Type-position sigil — decided.* `:(Functor (params) -> R)` paralleling
  `:(Function (args) -> R)`, Type-class token disjoint from the binder
  keyword.
- *Applicative-mode opt-in — deferred to predicate typing.* Tracked under
  [standard-library.md](../libraries/standard-library.md). Generative-only
  semantics ship under this item.
- *Where `is_functor` is set in fn_def — open.* The natural site is the
  same return-type classification arm that already routes
  `Resolved`/`Deferred`; deciding whether validation runs before or after
  the parameter-name scan affects which error fires first when a return
  type both references a parameter and fails the module/signature
  denotation check.
- *`KType::KFunctor` placement in `function_compat` — open.* Whether the
  cross-arm refusal is silent (matching today's strict-`==` failure mode)
  or surfaces a distinct `cannot pass a functor where a function is
  expected` message. Recommended: surface the message — the FUNCTOR
  binder is the only path that mints `KFunctor`, so a refusal here always
  points at programmer intent rather than coincidence.

## Dependencies

**Requires:**

**Unblocks:**

- [Standard library](../libraries/standard-library.md) — collections
  ship as FUNCTORs over their element/key types, so the FUNCTOR binder
  is the substrate for stdlib data-structure code.
