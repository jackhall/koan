# Roadmap

Open structural items that don't fit in a single PR. Each entry below names the problem,
why it matters, and possible directions — not a fixed design. Per-item write-ups live in
[roadmap/](roadmap/).

The order matters. Sequencing is purely about technical and design dependencies — Koan has
no users yet, so backward-compatibility costs play no role. The cost being optimized is
engineering rework: doing one item before another it depends on means doing the dependent
item twice. Each per-item file ends with a **Dependencies** section linking to its
prerequisites and the items it unblocks.

Design rationale for what's already in the language lives in [design/](design/) — six
topical docs covering the execution model, memory model, functional programming, type
system, expressions and parsing, and error handling. Two further design docs capture
cross-cutting work in flight: [design/module-system.md](design/module-system.md) — the
module-based abstraction system end-to-end (stages 1 and 2 shipped, remaining stages tracked
as `module-system-*` roadmap items below) — and [design/effects.md](design/effects.md)
— in-language monadic side effects (implementation tracked in
[roadmap/monadic-side-effects.md](roadmap/monadic-side-effects.md)). What's
shipped so far on the module-system and scheduler tracks: the dispatch-as-node
scheduler (every expression evaluates as a `Dispatch` node, so deferred work,
forward references, and cross-file references all reduce to the same
park-on-producer mechanism); the module-system stage 1 module language
(`MODULE` / `SIG` declarators, `:|` opaque and `:!` transparent ascription,
per-module type identity via `KType::UserType { kind: Module, scope_id, name }`,
and `Module` / `Signature` first-class values reachable via `Foo.member` ATTR
access); the dispatcher fold (overload resolution as one
`Scope::resolve_dispatch` chain walk returning a four-variant `ResolveOutcome`
whose `Resolved` carries the per-slot auto-wrap / replay-park / eager-sub
index buckets via `KFunction::classify_for_pick`); dispatch-time name
placeholders (binders install a `name → producer NodeId` entry in
`Scope::placeholders` at dispatch time so bare-identifier slot lookups whose
target binder has dispatched but not yet executed park on the producer instead
of failing with `UnboundName` — see [design/execution-model.md § Dispatch-time
name placeholders](design/execution-model.md#dispatch-time-name-placeholders));
the scheduler park-vs-own edge split (`DepEdge::Owned` / `DepEdge::Notify`
tagging so `free`'s recursive reclaim walks the ownership tree only and
ignores park edges installed by the single-Identifier short-circuit and
replay-park); and eager type elaboration end-to-end (one canonical runtime type
representation, scheduler-aware FN / STRUCT / UNION elaboration with
self-recursive STRUCT support and `LET Ty = Ty` cycle detection, FN
parameter slots written `(LIST_OF Number)` / `(DICT_OF Str Number)`
scheduling a sub-Dispatch from `parse_fn_param_list`, the `NoopResolver` /
`TypeResolver` / `ScopeResolver` seam plus the legacy
`parse_typed_field_list` deleted so scope-aware elaboration goes
exclusively through the scheduler-driven `elaborate_type_expr`,
module-qualified type names resolved through
[`access_module_member`](src/runtime/builtins/attr.rs)'s
`type_members` / `data` / `Scope::resolve_type` tier walk — so
`LET MyT = Mo.Ty` and chained `Outer.Inner.T` bind in type position —
and non-SCC forward type aliases like `LET Ty = Un; LET Un = Number`
parking on the producer's dispatch-time placeholder via the same rail
value-name forward references use, plus a head-Keyword fallback in
[`run_dispatch`'s `Unmatched` arm](src/runtime/machine/execute/scheduler/dispatch.rs)
that parks a consumer call on a sibling binder whose body deferred
through a Combine so its registration hasn't landed yet); and the type-identity stage 1 substrate
([`RuntimeArena::alloc_ktype`](src/runtime/machine/core/arena.rs), the
[`Bindings::types` map with the `try_register_type` and `try_register_nominal` write primitives](src/runtime/machine/core/bindings.rs),
the
[`Scope::register_type` rewire onto `bindings.types` plus the type-side `Scope::resolve_type` lookup API](src/runtime/machine/core/scope.rs),
and the [stage-1.5 consumer migration](src/runtime/builtins/value_lookup.rs)
that flips type-name reads onto `Scope::resolve_type` and deletes the
transient `Scope::resolve` fallback, plus the stage-1.6 bind-time diagnostic
[`KErrorKind::TypeClassBindingExpectsType`](src/runtime/machine/core/kerror.rs)
that rejects `LET <Type-class> = <non-type>` at the binder rather than at
downstream elaboration) — builtin type names live in
`bindings.types` as arena-allocated `&KType`, Type-token reads consult
`Scope::resolve_type` first (with the sole `KObject::KTypeValue` synthesis
site for dispatch transport now living in `value_lookup::body_type_expr`),
value-side nominal carriers (`KModule`, `StructType`, `TaggedUnionType`,
`KSignature`) fall through to `Scope::resolve` until stage 3 dual-writes a
`KType::UserType` next to them, and the LET `TypeExprRef`-LHS overload
routes `LET Ty = Number`-style aliases through `Scope::register_type` so
they live in `bindings.types` alongside the builtin type names — with
ascription's abstract-type member sweep walking both maps so SIG
abstract-type declarations stay visible across the storage split
([`ascribe.rs`](src/runtime/builtins/ascribe.rs)); and the type-identity
stage 2 carrier replacement
([`KObject::TypeNameRef(TypeExpr, OnceCell<&'a KType>)`](src/runtime/model/values/kobject.rs))
that lowers bare-leaf type names not in `KType::from_name`'s builtin table
on the value side at `resolve_for` time, memoizes the scope-resolved
`&'a KType` in the cell via
[`KObject::resolve_type_name_ref`](src/runtime/model/values/kobject.rs), and
deletes the placeholder `KType::Unresolved` variant so every `KType` flowing
through dispatch is fully elaborated; and the type-identity stage 3
per-declaration carrier and dual-write — the
[`KType::UserType { kind, scope_id, name }` per-declaration tag and `KType::AnyUserType { kind }` wildcard kind tag](src/runtime/model/types/ktype.rs)
(with the old `KType::Struct` / `Tagged` / `Module` / `ModuleType` singletons
deleted), the surface names `"Struct"` / `"Tagged"` / `"Module"` lowering to
the wildcard via
[`KType::from_name`](src/runtime/model/types/ktype_resolution.rs),
`(scope_id, name)` identity fields populated at finalize time on
[`KObject::Struct` / `Tagged` / `StructType` / `TaggedUnionType`](src/runtime/model/values/kobject.rs)
under the `scope as *const _ as usize` scheme `Module::scope_id()` uses,
predicate arms placing `UserType { kind: K, .. }` strictly under
`AnyUserType { kind: K }` strictly under `Any` in
[`ktype_predicates.rs`](src/runtime/model/types/ktype_predicates.rs),
`KObject::Struct` / `Tagged` / `KModule` synthesizing `KType::UserType`
from their identity fields in `ktype()`, STRUCT / UNION-named / MODULE /
SIG finalize routing through the
[`Scope::register_nominal`](src/runtime/machine/core/scope.rs) shim that
transactionally writes `bindings.types[name]` and `bindings.data[name]`
together (with `body_type_expr`'s value-side fall-through and the resolver's
`KSignature` / `StructType` / `TaggedUnionType` value-side fallback both
deleted under the single-home invariant), `LET <Type-class> = <module/sig/struct-value>`
aliases dual-writing through the same shim while preserving the original
carrier's identity, and the
[`Bindings.pending_types`](src/runtime/machine/core/bindings.rs)
SCC registry that closes mutually recursive STRUCT / named-UNION cycles by
pre-installing every member's identity into `bindings.types` so each
finalize hits `try_register_nominal`'s idempotent arm — with the
anonymous `UNION (...)` overload deleted so every tagged value carries a
real per-declaration identity; and the type-identity stage 4 `NEWTYPE`
keyword and [`KObject::Wrapped`](src/runtime/model/values/kobject.rs)
carrier — `NEWTYPE Distance = Number` mints a fresh nominal identity
([`KType::UserType { kind: Newtype { repr }, scope_id, name }`](src/runtime/model/types/ktype.rs))
over a transparent representation, `Distance(3.0)` constructs through
[`type_call`'s `Newtype` arm](src/runtime/builtins/type_call.rs) into
[`newtype_def::newtype_construct`](src/runtime/builtins/newtype_def.rs)
(an `add_dispatch` + `Combine` pair that type-checks against `repr` and
applies the construction-time newtype-over-newtype collapse so
`Wrapped.inner` is invariantly non-`Wrapped`), and ATTR over a
`Wrapped` carrier [falls through to `inner`](src/runtime/builtins/attr.rs)
via a new `AnyUserType { kind: Newtype { repr: Any } }` overload that
reuses `body_struct`'s `access_field` dispatch — so `b.x` on `LET b:
Boxed = Point(...)` reads the underlying struct's field without forcing
every accessor to redo; and the module-system stage-2 sharing-constraint
surface — the `SIG_WITH` builtin
([`type_ops.rs::body_sig_with`](src/runtime/builtins/type_ops.rs)),
[`KType::SignatureBound { pinned_slots }`](src/runtime/model/types/ktype.rs)
carrying the pins through admissibility and specificity, FN return-type
slots elaborating parens-wrapped `(SIG_WITH ...)` expressions via the
existing eager-sub-dispatch rails (with a `ReturnTypeCapture::TypeExpr`
carrier in [`fn_def.rs`](src/runtime/builtins/fn_def.rs) for the
Combine-boundary case), and MODULE-body finalize mirroring
`child_scope.bindings.types` into `Module::type_members`
([`module_def.rs`](src/runtime/builtins/module_def.rs)) so a body-side
`LET Elt = Number` admits the FN's declared
`(SIG_WITH SetSig ((Elt: Number)))` pin; and the module-system stage-2
higher-kinded type-constructor slot surface — the
[`TYPE_CONSTRUCTOR`](src/runtime/builtins/type_ops.rs) builtin returning
a template
[`KType::UserType { kind: UserTypeKind::TypeConstructor { param_names }, .. }`](src/runtime/model/types/ktype.rs),
[`elaborate_type_expr`'s constructor-application arm](src/runtime/model/types/resolver.rs)
emitting structural
[`KType::ConstructorApply { ctor, args }`](src/runtime/model/types/ktype.rs)
on `Wrap<Number>` against a `TypeConstructor`-resolved outer name (with
arity check and a placeholder-park rail mirroring the bare-leaf arm),
and [`ascribe.rs:body_opaque`](src/runtime/builtins/ascribe.rs)'s
per-call minting loop inspecting the SIG's `bindings.types` so an opaque
ascription against a SIG declaring `LET Wrap = (TYPE_CONSTRUCTOR Type)`
mints a fresh `TypeConstructor` slot per call — arity-1 only,
expressible end-to-end in a `SIG Monad = ((LET Wrap = (TYPE_CONSTRUCTOR
Type)) (LET pure = (FN (PURE a: Number) -> Wrap<Number> = (1))))`-shaped
signature. The next signature revision after error handling lands
monadic side-effect capture; the type-system arc runs through the
module-system stages — foundation now landed in stage 1, ergonomic generic
dispatch in stage 5, coherence in stage 6. And the module-system stage-2
audit-slate sign-off: the post-stage-1 Miri tree-borrows slate re-ran
clean across every unsafe site introduced by stage-2 substrate (opaque
ascription re-binds, type-op dispatch through the per-call arena),
closing the carry-forward; and the functor-params surface end-to-end —
parameter-position dual-write (the
[`KType::is_type_denoting`](src/runtime/model/types/ktype_predicates.rs)
predicate plus
[`KFunction::invoke`](src/runtime/machine/kfunction/invoke.rs)'s per-call
bind loop registering the per-call binding into `bindings.types`
alongside the existing `bind_value`, so a Type-class FN parameter
(`Er: OrderedSig`) is a type-language binder for body-position
references like `(MODULE_TYPE_OF Er Type)`) plus the templated
return-type surface (the
[`ReturnType<'a>` / `DeferredReturn<'a>` carriers at
`ExpressionSignature::return_type`](src/runtime/model/types/signature.rs)
routing parameter-name-bearing return types through deferred per-call
elaboration in `KFunction::invoke`'s Combine-finish closure, with the
parens-form return-type overload in
[`fn_def.rs`](src/runtime/builtins/fn_def.rs) registering its
return-type slot as `KType::KExpression` so `(MODULE_TYPE_OF Er Type)`
and `(SIG_WITH Set ((Elt: Er)))` survive FN-def without sub-dispatching
against the outer scope), with the FN-param parser
([`fn_def/signature.rs`](src/runtime/builtins/fn_def/signature.rs))
relaxed to admit Type-classified bare-leaf tokens in the parameter-name
slot of the `<name>: <Type>` triple; and the SIG-slot explicit-type
ascription surface — the `VAL <name>: <TypeExpr>` declarator
([`val_decl.rs`](src/runtime/builtins/val_decl.rs)) is the canonical
value-slot declaration inside a SIG body, replacing the
ascription-by-example `(LET name = <value>)` form (rejected inside SIG
bodies with a diagnostic directing to `VAL`), with the slot's declared
type recorded as a `KType::KTypeValue` carrier under the SIG decl_scope's
`bindings.data` so the existing name-presence shape check in
[`ascribe.rs`](src/runtime/builtins/ascribe.rs) admits any supplying
member uniformly, full type-shape checking against the declared slot type
deferred to [Modular implicits](roadmap/module-system-5-modular-implicits.md).
Unblocks standard-library collection functors (`Make` over `ORDERED`)
and dependent parameter annotations.

## Next items

Items with no unresolved roadmap-level prerequisites — any of these can be picked up
without first landing something else:

- [Files and imports](roadmap/files-and-imports.md) — wire `.koan` files together so a
  codebase can span more than one source file and files become modules.
- [Simplify `runtime::machine` and shrink AI context cost](roadmap/simplify-and-shrink-context.md)
  — `runtime::machine` owns ~60% of the crate's fractal coupling index and three
  non-test files exceed 600 lines; score reshuffles via `modgraph_rewrite.py`,
  split the largest files, then trim scheduler tests the sub-struct extractions
  made redundant.

## Open items

### Memory and runtime substrate

- [Generalize `Scope::out` into monadic side-effect capture](roadmap/monadic-side-effects.md)
  — replace the ad-hoc `Box<dyn Write>` with an in-language `Monad` signature
  (see [design/effects.md](design/effects.md)) plus a runtime `Effectful<T>` carrier;
  ships standard effect modules (`Random`, `IO`, `Time`). The `Wrap` slot's
  higher-kinded surface (`(TYPE_CONSTRUCTOR Type)`) has landed via module-system
  stage 2.

### Module system

The agreed design is captured in [design/module-system.md](design/module-system.md);
stages 1 and 2 shipped (the module language: `MODULE`/`SIG` declarators,
`:|`/`:!` ascription, per-module type identity, plus the scheduler-driven
elaborator, `SIG_WITH` sharing constraints, higher-kinded
type-constructor slots, and the post-stage-1 Miri audit-slate
carry-forward), and the remaining stages below land the rest
incrementally, each producing a usable end state.

- [Dependent parameter annotations](roadmap/module-system-dependent-param-annotations.md) —
  parameter type slots that reference earlier parameters in the same FN
  signature (`(MAKE T: Type elt: T)`, OCaml's
  `module Make (E : ORDERED) (S : SET with type elt = E.t)`). Reuses
  the `ReturnType` / `DeferredReturn` carrier shipped at
  [`ExpressionSignature::return_type`](src/runtime/model/types/signature.rs)
  and the per-call re-elaboration plumbing in
  [`KFunction::invoke`](src/runtime/machine/kfunction/invoke.rs); the new
  work is staged left-to-right dispatch.
- [VAL-slot value-carrier abstract-identity tagging](roadmap/val-slot-abstract-identity-tagging.md)
  — a value read from an `:|`-ascribed module's VAL-declared slot today
  carries the underlying value's `KType`, not the per-call abstract
  identity `:|` minted for the SIG's `Type` member; closes the
  deferred end-to-end functor-on-VAL-slot call test variant in
  [`functor_return_module_type_of_parameter_resolves_per_call`](src/runtime/builtins/fn_def/tests/module_stage2.rs)
  and aligns dispatch keys for stage 5's implicit search over
  VAL-typed values.
- [Stage 4 — Property testing and axioms](roadmap/module-system-4-axioms-and-generators.md)
  — Rust-side property-testing engine kept disjoint from dispatch; axiom syntax in
  signatures with compile-time checking on ascription.
- [Stage 5 — Modular implicits](roadmap/module-system-5-modular-implicits.md) —
  implicit module parameters with lexical resolution and strict-on-ambiguity.
- [Stage 6 — Equivalence-checked coherence](roadmap/module-system-6-equivalence-checking.md)
  — cross-implicit equivalence testing; the differentiating coherence story.
- [Stage 7 — Syntax tuning and witness types](roadmap/module-system-7-syntax-tuning.md)
  — disambiguation sugar designed against patterns from real stage-5 code, plus opt-in
  witness types.

### Type system

- [Group-based operators](roadmap/group-based-operators.md) — `+`/`-` form a math group
  but the language treats every operator as a flat independent builtin. Generic
  dispatch over groups arrives with the module system's modular implicits.
- [Structural KFunction admission across deferred return types](roadmap/kfunction-deferred-ret-precision.md) —
  [`function_value_ktype`](src/runtime/model/values/kobject.rs) synthesizes
  `KType::KFunction { ret: KType::Any }` for deferred-return FNs because the
  structural function-type language has no surface for "per-call
  elaboration of this expression"; the symmetric coarsening in
  [`function_compat`](src/runtime/model/types/ktype_predicates.rs) admits-or-
  rejects-by-`==` so today's strict refusal stays safe but silent. A
  `debug_assert!` at the coarsening branch is the tripwire; the decision
  is forced when stage 5 implicit search or a precise FN-typed slot
  ascription first exercises the scenario.

### Surface and ergonomics

- [Files and imports](roadmap/files-and-imports.md) — a Koan codebase is one file;
  no way for a `.koan` file to reach into another, and no story for how files become
  modules.
- [Error-handling surface follow-ups](roadmap/error-handling.md) — errors-as-values,
  source spans on `KExpression`, continue-on-error (independent), plus typed
  user errors and the catch surface (gated on module-system stage 2).
- [Standard library](roadmap/standard-library.md) — collections (`Set`, `Map`,
  …) and standard effect modules (`Random`, `IO`, `Time`) ship as Koan-source
  functor FNs across multiple `.koan` files; doubles as the canonical example
  of idiomatic module / signature / functor / import composition.

### Future-facing

- [Static type checking and JIT compilation](roadmap/static-typing-and-jit.md) — the
  tooling and performance ceiling; both want a phase between parse and execution.
