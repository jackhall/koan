# FUNCTOR binder

Bring the runtime in line with [design/typing/functors.md](../../design/typing/functors.md):
a dedicated `FUNCTOR` binder with definition-time return-type validation, a
distinct `KType::KFunctor` variant with a one-way admissibility wall
against `KType::KFunction`, and a `:(Functor (params) -> R)` type-position
sigil. Two known issues are folded into this work because they share its
machinery and code path.

**Problem.** Functors are not a first-class binder. An FN whose return
slot happens to denote a module or signature is the only way to write
one, so functor-ness must be inferred from the resolved (or deferred)
return-type carrier at every consumer. Concrete fallout in the runtime
today:

- [`function_value_ktype`](../../src/machine/model/values/kobject.rs)
  and [`function_compat`](../../src/machine/model/types/ktype_predicates.rs)
  coarsen `Deferred(_)` returns to `KType::Any` — the structural
  function-type language has no surface for "this FN is a functor and its
  return slot is module-kind," tracked under
  [kfunction-deferred-ret-precision.md](kfunction-deferred-ret-precision.md).
- No FN-def-time check forces the return slot of an intended functor to
  denote a module or signature; mistakes surface as opaque dispatch-time
  errors several frames removed from the binder.
- *Defining a functor panics the scheduler at the CLI seam.* The
  `cargo run` smoke
  `SIG OrderedSig = (VAL compare :Number)\nFN (MAKESET Er :OrderedSig) -> OrderedSig = (Er)`
  panics at
  [`node_store.rs:169`](../../src/machine/execute/scheduler/node_store.rs)'s
  `read_result` during interpret's per-top-level result print. The
  scheduler's eager-free policy in `reclaim_deps` frees the SIG
  dispatch's slot when the FN-def's Combine succeeds, and interpret's
  subsequent `read_result(top_level_id)` hits a `Free` slot. Tracked
  separately under
  [Scheduler eager-free policy vs. interpret top-level read-back](../scheduler-reclaim-vs-interpret-readback.md);
  the FUNCTOR binder cannot land its end-to-end smoke until that path
  runs clean.
- *Type-class `LET` gate is a denylist — plain values slip into a type
  name.* The `TypeClassBindingExpectsType` check
  ([`let_binding.rs:51`](../../src/builtins/let_binding.rs) / `:92`) only
  rejects `Number | Str | Bool | Null | List | Dict`; any other value
  passes through to `bind_value` into `data` with no error, so
  `LET Plain = (FN (PP x :Number) -> Number = (x))` silently binds a
  plain function under a Type-class name. The discrimination needed for
  an allowlist — separating a functor from a plain function when both
  are `KObject::KFunction` — is exactly what the `is_functor` flag
  provides, so the gate fix rides this work.

There is also no type-position surface for "this slot is a functor of this
shape" — only the binder keyword exists, so functor-returning functors
have no admissible return-type denotation.

**Impact.**

- *FUNCTOR return slots are statically validated at the binder.* The
  admissible carriers (`KType::AnySignature`, `SatisfiesSignature`,
  `(SIG_WITH …)`, `KType::AnyModule`, `KType::Module { .. }`,
  `KType::Signature(_)`, recursively `KType::KFunctor`) are checked
  when the FUNCTOR binder runs; any other denotation surfaces
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
- *Functor definitions stop panicking at the CLI seam.* Riding on the
  scheduler-reclaim fix
  ([Scheduler eager-free policy vs. interpret top-level read-back](../scheduler-reclaim-vs-interpret-readback.md)),
  the FUNCTOR end-to-end smoke runs through `cargo run` without the
  `read_result` panic on the interpret seam.
- *Type-class LET gate flips to an allowlist.* The accepted RHSs become
  `KTypeValue`, `derive_nominal_identity → Some`, or an
  `is_functor`-flagged `KFunction`; plain functions bound to Type-class
  names get rejected with a real diagnostic instead of silently landing
  in `data`.
- *FUNCTOR is the declared seam for future applicative-mode work.*
  Generative-only semantics ship now; once predicate typing lands, the
  applicative opt-in (parked in [standard-library.md](../libraries/standard-library.md))
  attaches to the `is_functor` flag rather than re-deriving functor-ness
  from the return type at every consumer.

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
- *Type-class LET gate flips to allowlist — decided.* Accept iff RHS is
  `KTypeValue`, `derive_nominal_identity → Some`, or
  `is_functor`-flagged. The current denylist's "hard part" (functor /
  plain-function discrimination) dissolves once the flag exists.
- *Applicative-mode opt-in — deferred to predicate typing.* Tracked
  under [standard-library.md](../libraries/standard-library.md). Generative-only
  semantics ship under this item.
- *Functor-definition panic root cause — decided.* Attributed to the
  scheduler's eager-free policy in `reclaim_deps` (not the type-language
  layer), and tracked separately under
  [Scheduler eager-free policy vs. interpret top-level read-back](../scheduler-reclaim-vs-interpret-readback.md).
  The FUNCTOR binder work picks up after that fix lands.
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

- [Scheduler eager-free policy vs. interpret top-level read-back](../scheduler-reclaim-vs-interpret-readback.md)
  — the CLI smoke for a signature-typed FUNCTOR parameter trips the
  interpret seam's `read_result` panic; that fix unblocks the FUNCTOR
  binder's end-to-end test.

**Unblocks:**

- [Standard library](../libraries/standard-library.md) — collections ship as FUNCTORs
  over their element/key types, so the FUNCTOR binder is the substrate
  for stdlib data-structure code.
