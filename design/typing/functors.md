# Functors

A **functor** is a module parameterized by another module — a function from
modules to modules. Koan presents this through a dedicated `FUNCTOR` binder
that layers definition-time static guarantees over the same per-call dispatch
machinery ordinary FNs use.

- *Surface semantics* — modules are part of the **type language**. A
  signature-typed FUNCTOR parameter (`Er: OrderedSig`) is a type-language
  binder, like an OCaml functor's parameter. `Er.Type` in a type-position
  slot is type-language projection — extracting the module's abstract
  type. Identifier-class names (`er`, `mo` — lowercase-first per
  [tokens.md](tokens.md)) are value-language only and a hard error in any
  type-position slot.
- *Machine semantics* — modules are **first-class values**.
  `KObject::KTypeValue(KType::Module { module, frame })` flows through the
  scheduler like any other value (the same `KTypeValue` carrier `Number`,
  `Str`, and other type values ride), and a FUNCTOR is internally an
  ordinary `KFunctionValue` with an `is_functor` flag set at binder time.
  The flag drives two separable effects:
  definition-time validation of the return-type slot, and a distinct
  `KType::KFunctor { params, ret }` surfaced by the value's `ktype()`. The
  dispatch path, scheduler integration, per-call scope, and `KFunction::invoke`
  are unchanged — FUNCTOR is a thin definition-time façade over FN mechanics.
  Type-position references to functor types use the `:(Functor (params) -> R)`
  sigil — a Type-class token paralleling `:(Function (args) -> R)` — kept
  surface-disjoint from the `FUNCTOR` binder keyword on the same rule that
  keeps `FN` (binder) and `Function` (type) disjoint.

```
LET MakeSet = (FUNCTOR (MAKESET Er :OrderedSig) -> SetSig = (
  MODULE Result = (
    (LET Type = ...)
    (LET insert = (FN (INSERT s :Type x :Er.Type) -> Type = ...))
    ...
  )
))

LET IntSet = (MAKESET IntOrd)
```

`MODULE Name = (...)` is itself an expression: it both binds `Name` in the
enclosing per-call scope and evaluates to the module value, so the FUNCTOR
body needs no separate "anonymous structure" form. The bound name (`Result`
above) lives only inside the call frame.

`FUNCTOR` and `FN` are surface-disjoint. An FN whose body happens to evaluate
to a module value is **not** a functor: it has no `is_functor` flag, its
`ktype()` is `KType::KFunction`, and none of the functor-specific definition-
or dispatch-time machinery (return-type validation, applicative-mode
eligibility) applies. The programmer always knows whether they are writing
a functor; the binder makes that knowledge legible to the engine.

## Definition-time validation

FUNCTOR's return-type slot must denote a module, signature, or functor
kind. The admissible carriers are `KType::AnySignature`, `SatisfiesSignature`,
`(SIG_WITH …)`, `KType::AnyModule`, `KType::Module { .. }`,
`KType::Signature(_)`, and `KType::KFunctor { … }` (recursively — the
inner `ret` is validated the same way, so curried multi-module functors
and any deeper nesting flow through one rule). Any
other denotation — `Number`, a structural function type, a plain user
type — is a definition-time error at the FUNCTOR binder, surfaced with
`FUNCTOR return-type slot must denote a module, signature, or functor`
wording. FN imposes no such constraint.

The same parameter-name scan that classifies an FN return type into
`Resolved` / `Deferred` runs for FUNCTOR; the validation gates on the
denotation of the resolved or deferred carrier. A return type like
`(SIG_WITH Set ((Elt: Er)))` that references a per-call parameter is
admissible because the outer carrier (`SIG_WITH`) is a signature constructor;
the `Er` reference resolves through the per-call `bindings.types` write at
dispatch.

## Type identity and the one-way wall

`KType::KFunctor { params, ret }` is a distinct structural variant. The
admissibility helper at [`function_compat`](../../src/machine/model/types/ktype_predicates.rs)
matches `KFunctor → KFunctor` on the same structural rules used for
`KFunction → KFunction`, but refuses both directions of the
`KFunctor`/`KFunction` cross — a functor cannot be passed where a function
is expected, and vice versa. The wall lives entirely at the type-admission
layer; the underlying `KFunctionValue` is shared.

This rules out the surface-level confusion of "I have a value that returns
a module, can I pass it to something expecting a functor?" — the answer is
no: rebind it as a FUNCTOR if that's the intent.

## Generativity

FUNCTOR application is **generative**: each call evaluates the body afresh,
and any inner `:|` mints fresh `KType::AbstractType { source_module, name }`
slots. `(MAKESET IntOrd)` applied twice yields two distinct `Set` types
that cannot be confused. Generativity is a consequence of `:|`-per-call;
the mechanism is general (any FN that contains `:|` mints fresh slots on
each call) and not FUNCTOR-specific.

An **applicative** variant — same-functor-applied-to-same-module producing
the same output types, so independent call sites resolving to the same
implicit module interoperate — is deferred behind the predicate-typing
work. The language stays generative-only until that substrate lands.
Routing applicative-mode through FUNCTOR (rather than FN) when it does land
keeps the generative/applicative choice visible at the declaration. See
[open-work.md](open-work.md).

## Sharing constraints

Sharing constraints — pinning a functor's output abstract type to a
specific concrete type — ride on the `SIG_WITH` builtin described in
[Type expressions and constraints](#type-expressions-and-constraints). A
FUNCTOR whose return type is `(SIG_WITH SetSig ((Elt: Number)))` declares
the constraint at the return slot; the body's `MODULE Result` must mirror
`Elt = Number` for the return-type check to admit it. There is no separate
`with type` keyword.

Pin values that reference only the FUNCTOR's outer scope are elaborated at
binder-construction time. Concrete builtins (`Number`, `Str`) and
outer-scope-bound type values (`(MODULE_TYPE_OF Mo Type)` where `Mo` is
bound outside the FUNCTOR) both work as pin values resolved eagerly.

## Parameters

A Type-class FUNCTOR parameter (`Er: OrderedSig`) binds the parameter name as
a type-language binder at the call site: at each call,
[`KFunction::invoke`](../../src/machine/core/kfunction/invoke.rs)
writes the per-call argument into the child scope's `bindings.types` only
(not `bindings.data`). The
[`KType::is_type_denoting`](../../src/machine/model/types/ktype_predicates.rs)
predicate gates the write — `SatisfiesSignature`, `Type`, `TypeExprRef`,
`KType::AnyModule`, and `KType::AnySignature` all carry meaningful
type-language identity at the binder, and the corresponding argument is
admitted as a single carrier shape. Body-position references to the
parameter (`Er.compare`, `(MODULE_TYPE_OF Er Type)`) resolve through
`Scope::resolve_type`'s outer-chain walk against the per-call scope, and
[`attr.rs`](../../src/builtins/attr.rs)'s `body_identifier` arm falls
through to `resolve_type` for `KType::Module` / `AbstractType` so ATTR on
a signature-typed parameter projects through the type-side carrier.

FUNCTOR parameters are otherwise **unrestricted ordinary FN parameters**.
Because koan unifies the value and module languages — a module is a
first-class `KObject::KTypeValue(KType::Module { .. })`, a FUNCTOR an
`is_functor`-flagged `KFunctionValue` — a FUNCTOR parameter can be
anything an FN parameter can be, including a bare value
(`FUNCTOR (MAKETREE factor :Number) -> …`, with
the body's `MODULE` closing over `factor` lexically). This is where koan
departs from OCaml: OCaml stratifies a separate module language above the
value language, so a functor takes only module arguments and a value must be
smuggled in via `struct let factor = 4 end`. Koan has no such stratum, so a
value parameter binds directly — no wrapping, and no requirement that any
FUNCTOR parameter be signature-typed. A value passed this way is **runtime
data, not part of type identity**: per-call generativity still mints fresh
abstract types each call, but two calls differing only in the value produce
structurally identical type members. Koan has no type-level values, so a
value parameter never enters type identity; const-generic-style
parameterization, where the value *is* part of the type, is a different
model koan does not adopt.

The same no-stratum reasoning extends symmetrically to bare type tokens. A
`:Type`-typed parameter slot admits any `KTypeValue`-carried type — bare
builtin tokens (`Number`, `Str`, `Bool`, `Null`) and the
`KTypeValue(KType::UserType { .. })` carrier a struct / union nominal token
synthesizes on demand — so `(MAKETREE Number)` against
`FUNCTOR (MAKETREE Elt :Type) -> …` binds `Elt = KType::Number` per call
with no call-site wrapping. The per-call type-side bind treats the
builtin-keyed and nominal-keyed paths identically: a body-position `Elt`
resolves to `KType::Number` through `Scope::resolve_type`, and a deferred
return like `-> :Elt` re-elaborates through the same Combine-finish slot
check the nominal-keyed path uses. The wall on `KType::Module { .. }` /
`KType::Signature(_)` carriers stays in place — those route through
`AnyModule` / `AnySignature` / `SatisfiesSignature` slots, keeping the
`:Type` vs `:Module` overload distinction. OCaml structurally cannot match
this without modular implicits, because its module language is stratified
above the value language.

## Deferred return-type elaboration

Return-type expressions that reference a per-call FUNCTOR parameter
(`-> Er`, `-> (MODULE_TYPE_OF Er Type)`, `-> (SIG_WITH Set ((Elt: Er)))`)
ride a *deferred* return-type carrier through the per-call scope.
[`ExpressionSignature::return_type`](../../src/machine/model/types/signature.rs)
is a `ReturnType<'a>` enum, not a bare `KType`: `Resolved(KType)` covers
every static case (return types that don't reference a parameter), while
`Deferred(DeferredReturn<'a>)` holds the surface form verbatim — either
`TypeExpr(TypeExpr)` for parser-preserved structured forms or
`Expression(KExpression<'a>)` for captured parens-form expressions. Routing
happens at binder construction in
[`fn_def.rs`](../../src/builtins/fn_def.rs): a parameter-name scan over the
captured return-type carrier picks `Deferred(_)` when any leaf matches a
parameter name and `Resolved(_)` otherwise. The parens-form overload
registers its return-type slot as `KType::KExpression` so the expression
survives binder definition without sub-dispatching against the outer scope.

Per-call elaboration runs at the dispatch boundary in
[`KFunction::invoke`](../../src/machine/core/kfunction/invoke.rs). The
`Deferred(_)` arm spawns the body Dispatch and (for the `Expression`
carrier) an optional return-type sub-Dispatch under the per-call frame
via `SchedulerHandle::with_active_frame` (see
[per-call-arena-protocol.md § Active-frame propagation](../per-call-arena-protocol.md#active-frame-propagation)),
then joins them in a `Combine` whose finish closure runs
`per_call_ret.matches_value(body_value)` and surfaces mismatches with
`(per-call return type)` wording. The
`TypeExpr` carrier elaborates inline against the per-call scope where
the per-call type-side bind has installed the parameter-name
identities; both carriers feed the same Combine. The inline elaboration
is the standard
[elaboration.md § Layers](elaboration.md#layers) § Layer 3 walk against
the per-call scope. The lift-time return-type check in
[`scheduler/execute.rs`](../../src/machine/execute/scheduler/execute.rs)
gates on `ReturnType::is_resolved()` so the static-typing pathway stays
untouched and the deferred slot check runs only inside the Combine
finish where the per-call elaboration is in hand. The structural
`KType::KFunctor { ret }` synthesis at
[`function_value_ktype`](../../src/machine/model/values/kobject.rs) and the
admission helper at
[`function_compat`](../../src/machine/model/types/ktype_predicates.rs)
coarsen `Deferred(_)` to `KType::Any` because the structural function-type
language has no surface for "per-call elaboration of this expression" —
see [open-work.md](open-work.md) for the precision refinement.

Multi-argument FUNCTORs are ordinary multi-parameter binders. Currying is
just nested FUNCTORs whose outer return type is the inner functor's type,
written with the `:(Functor (params) -> R)` sigil:

```
LET MakeMap = (FUNCTOR (MAKEMAP Er :OrderedSig)
                -> :(Functor (Vo :MonoidSig) -> (SIG_WITH Map ((Key: Er.Type)))) = (
  FUNCTOR (Vo :MonoidSig) -> (SIG_WITH Map ((Key: Er.Type))) = (
    MODULE Result = ( ... )
  )
))
```

The outer return type is admitted by the recursive `KFunctor` arm in
[Definition-time validation](#definition-time-validation); the inner functor
inherits the outer's per-call scope, so `Er.Type` in its return slot resolves
through the same per-call type-side bind path body-position references use.

## Higher-kinded type slots

Signatures can declare **type-constructor slots** — abstract types that take
a type parameter — so parametric abstractions like the `Monad` signature in
[design/effects.md](../effects.md) are expressible:

```
SIG Monad = (
  (LET Wrap = (TYPE_CONSTRUCTOR Type))
  (VAL pure :(Function (Number) -> :(Wrap Number)))
  (VAL bind :(Function (:(Wrap Number), :(Function (Number) -> :(Wrap Number))) -> :(Wrap Number)))
)
```

`(TYPE_CONSTRUCTOR <param>)` is the declaration form: inside a SIG body it
binds the slot name (`Wrap` above) to a template
`KType::UserType { kind: UserTypeKind::TypeConstructor { param_names }, .. }`
carrying the parameter symbol list. The builtin lives in
[`type_ops.rs`](../../src/builtins/type_ops.rs).

Application uses the type-expression sigil:
`:(Wrap Number)` in a type-position slot elaborates through
[`elaborate_type_expr`](../../src/machine/model/types/resolver.rs)'s
constructor-application arm into
`KType::ConstructorApply { ctor: <the Wrap UserType>, args: [Number] }` —
structural identity by `(ctor, args)`, mirror of `List(_)` / `Dict(_, _)`.
The arm arity-checks against the constructor's `param_names.len()` and
parks on a placeholder when the outer name is an in-flight `LET`, the same
forward-reference path bare-leaf type names use.

Higher-kinded slots are **per-call generative on the same path as ordinary
abstract type slots**. Two opaque ascriptions of the same source module
against the same SIG mint distinct `TypeConstructor` carriers under each
resulting module's `type_members[Wrap]` — their `(scope_id, name)` pairs
differ, so `First.Wrap<Number>` and `Second.Wrap<Number>` are incomparable
types. The minting site is the same loop in `ascribe.rs:body_opaque` that
mints `KType::AbstractType` slots; it inspects the SIG's
`bindings.types[<slot>]` and matches `UserTypeKind::TypeConstructor` so the
slot inherits its declared kind (falling back to `AbstractType` for plain
`LET Type = ...` slots).

The surface is **arity-1 only.** The `param_names` list always carries one
entry; multi-parameter constructors (`Functor F G`) are tracked in
[open-work.md](open-work.md). The parameter symbol must be a Type-classified
token (≥1 lowercase character): the parser rejects single-letter capitals
(`T`, `E`) at lex time, so surface forms in this section using `T` are
conceptual — real code writes `(TYPE_CONSTRUCTOR Type)` or
`(TYPE_CONSTRUCTOR Elt)`. The [token-class rule](tokens.md) is the
parser-level cause.

`ConstructorApply` is a type-language-only variant: no `KObject` reports a
`ConstructorApply` `ktype()`. The variant flows through the type-position
machinery (FN return-type elaboration, signature-body ascription) and the
value-level admissibility — wrapping a concrete value in `Wrap<Number>` and
unwrapping it — and cross-module application (`M.Wrap<Number>` reached via
ATTR-then-apply) are tracked in [open-work.md](open-work.md). Bare `Wrap<T>`
in a signature body or against a root-scope-bound constructor is the path
the test suite pins.

## Type expressions and constraints

The `:(...)` type-expression sigil parameterizes `:(List T)`, `:(Dict K V)`,
and `:(Function (args) -> R)`
([ktype.md § Container type parameterization](ktype.md#container-type-parameterization))
for positional structural types. Sharing constraints,
modular-implicit signature constraints, and witness-typed
instantiations ride on a separate **parens-form builtin family** that
reuses the `name: value` triple shape FN parameters and STRUCT fields
use. The two surfaces stay disjoint: `:(...)` for structural shapes whose
slot semantics are positional, parens-form builtins for slot-named
constraints.

- **`SIG_WITH`.** Pins abstract type slots of a signature to specific
  concrete types. `(SIG_WITH OrderedSig ((Type: Number)))` is
  `OrderedSig` with its `Type` slot pinned to `Number`;
  `(SIG_WITH Set ((Elt: Number) (Ord: IntOrd)))` pins multiple slots in
  one call. The inner parens groups are each one `name: value` triple,
  matching the shape FN parameters parse.
- **Type-valued slot values.** `SIG_WITH` slot values accept any
  expression that evaluates to a `KType`, not only bare type-name
  tokens. `(SIG_WITH MySig ((Elt: (MODULE_TYPE_OF Mo Type))))`
  works because `MODULE_TYPE_OF` returns the abstract type of module
  `Mo`. The slot's declared kind decides what the engine expects.
- **Module-kind slots.** Type constructors can declare slots that take
  modules. `(SIG_WITH Set ((Elt: Number) (Ord: IntOrd)))` works because
  `Set`'s `Ord` slot is declared `OrderedSig`-kind. Distinct module
  values bound to the same slot give distinct concrete types — the
  mechanism behind witness types in
  [open-work.md](open-work.md).

Sharing constraints, modular-implicit signature constraints, and
witness-typed instantiations share this one builtin family. The
implicit *marker* itself (which parameter is implicit) is orthogonal —
see [implicits.md](implicits.md).
