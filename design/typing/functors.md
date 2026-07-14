# Functors

A **functor** is a module parameterized by another module — a function from
modules to modules. Koan presents this through a dedicated `FUNCTOR` binder
that layers definition-time static guarantees over the same per-call dispatch
machinery ordinary FNs use. A functor may also take a bare `:Type` parameter;
generic functions are built this way — see [generics.md](generics.md).

- *Surface semantics* — modules are part of the **type language**. A
  signature-typed FUNCTOR parameter (`Er: OrderedSig`) is a type-language
  binder, like an OCaml functor's parameter. `Er.Type` in a type-position
  slot is type-language projection — extracting the module's abstract
  type. Identifier-class names (`er`, `mo` — lowercase-first per
  [tokens.md](tokens.md)) are value-language only and a hard error in any
  type-position slot.
- *Machine semantics* — modules are **first-class values**.
  `KObject::Module(&Module)` flows through the
  scheduler in the value channel's `Object` arm like any other value (a signature,
  by contrast, rides the [`Carried::Type`](../../src/machine/model/values/carried.rs)
  arm alongside `Number`, `Str`, and other type values), and a FUNCTOR is internally an
  ordinary `KFunctionValue` with an `is_functor` flag set at binder time.
  The flag drives two separable effects:
  definition-time validation of the return-type slot, and a distinct
  `KType::KFunctor { params, ret, body }` surfaced by the value's `ktype()`. The
  dispatch path, scheduler integration, per-call scope, and body executor
  (`run_user_fn`) are the same as a plain FN — FUNCTOR is a thin definition-time façade over FN mechanics.
  `is_functor` never touches the call path: head-position functor application
  reuses the ordinary function-call convention (see
  [Application and binding](#application-and-binding)).
  Type-position references to functor types use the `:(FUNCTOR (params) -> R)`
  sigil — a Type-class token paralleling `:(FN (args) -> R)` — kept
  surface-disjoint from the `FUNCTOR` binder keyword by the `:(...)` sigil
  context, the same way `:(FN ...)` in type position is disjoint from the
  bare `FN` binder.

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

## Application and binding

A functor name-binding lives in the **type namespace**. A
`LET MakeSet = (FUNCTOR …)` against a Type-class (capitalized) name registers
in `bindings.types` as a `KType::KFunctor { body: Some(f) }` — the carried
callable `&KFunction` is what a later application invokes. This type-side home
is what lets a `Type` head and the `:(…)` sigil resolve a functor at all.
Binding a functor to a lowercase (value-class) name is an **error** at the LET
site, so `bindings.data` is unconditionally functor-free (see
[elaboration.md § Binding-map partition](elaboration.md#binding-map-partition)).

Head-position application reuses the function-call convention with no separate
machinery — `apply_callable`'s `Function` arm calls the carried `&KFunction` by
name, the same arm a plain function call takes, and the result happens to be a
module. Two application surfaces reach it:

- `MyFunctor {T = IntOrd}` — a `Type`-head `TypeCall`. The leaf name resolves to
  the `KType::KFunctor { body: Some }` type-table entry, which classifies as a
  callable function.
- `:(MyFunctor {T = IntOrd})` — a single-part `:(…)` sigil whose inner expression
  tail-dispatches the same `Type`-head `TypeCall`. A `:(…)` head *followed by* a
  call body is instead the `TypeHeadDeferred` lane, which evaluates the head to a
  type-shaped value and admits only a constructible type or a functor.

The classification machinery for these lanes is owned by
[execution/name-placeholders.md § Dispatch-time name placeholders](../execution/name-placeholders.md#dispatch-time-name-placeholders);
the functor/function distinction survives only at classification (for the
`KFunctor` typing and the `TypeHeadDeferred` diagnostic gate), never at
execution.

## Definition-time validation

FUNCTOR's return-type slot must denote a module, signature, or functor
kind. The admissible carriers are `KType::OfKind(KKind::Signature)`,
`KType::Signature { .. }` (the unified constraint-and-value variant, covering a bare
`:OrderedSig`, a `(… WITH {…})` pin, the `:Module` surface keyword's empty signature,
and a bare module head's `SelfOf` self-sig — a concrete module return lands here
because a module value's `ktype()` *is* its self-sig), and `KType::KFunctor { … }`
(recursively — the inner `ret` is validated the same way, so curried
multi-module functors and any deeper nesting flow through one rule). Any
other denotation — `Number`, a structural function type, a plain user
type — is a definition-time error at the FUNCTOR binder, surfaced with
`FUNCTOR return-type slot must denote a module, signature, or functor`
wording. FN imposes no such constraint.

The same parameter-name scan that classifies an FN return type into
`Resolved` / `Deferred` runs for FUNCTOR; the validation gates on the
denotation of the resolved or deferred carrier. A return type like
`:(Set WITH {Elt = Er})` that references a per-call parameter is
admissible because the outer carrier (`WITH`) is a signature constructor;
the `Er` reference resolves through the per-call `bindings.types` write at
dispatch.

## Type identity and the one-way wall

`KType::KFunctor { params, ret }` is a distinct structural variant.
`params` is a name-keyed [parameter `Record<KType>`](ktype/records-and-limits.md#record-fields-and-ktype-hashing) —
the same substrate `KFunction` uses — so a functor's parameter names (including
capitalized `Type`-token names like `Ty` / `Er`) are part of its identity and
round-trip through `KType::name()`. Identity is the record's order-blind
equality: `:(FUNCTOR (T :Sig, U :Sig2) -> M)` equals the same two parameters
declared in either order. The admissibility helper at
[`function_compat`](../../src/machine/model/types/ktype_predicates.rs)
matches `KFunctor → KFunctor` on the same function-subtyping rules used for
`KFunction → KFunction` — contravariant params with width-drop, covariant
return (see [ktype/parameterization-and-variance.md § Variance](ktype/parameterization-and-variance.md#variance)) — but refuses both
directions of the `KFunctor`/`KFunction` cross — a functor cannot be passed
where a function is expected, and vice versa. The wall lives entirely at the type-admission
layer; the underlying `KFunctionValue` is shared. `KType::join` mirrors the
wall: it joins two same-shape `KFunctor`s to a shared `KFunctor` (so a list
literal of same-shape functors infers `List<:(FUNCTOR …)>`) and two
`KFunction`s to a shared `KFunction`, but a function joined with a functor
falls through to `Any`.

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
specific concrete type — ride on the `WITH` builtin described in
[Type expressions and constraints](#type-expressions-and-constraints). A
FUNCTOR whose return type is `:(SetSig WITH {Elt = Number})` declares
the constraint at the return slot; the body's `MODULE Result` must mirror
`Elt = Number` for the return-type check to admit it. There is no separate
`with type` keyword.

Pin values that reference only the FUNCTOR's outer scope are elaborated at
binder-construction time. Concrete builtins (`Number`, `Str`) and
outer-scope-bound type values (`Mo.Type` where `Mo` is
bound outside the FUNCTOR) both work as pin values resolved eagerly.

## Parameters

A FUNCTOR parameter binds per-call in whichever channel its argument travels.
A **module** argument — what a `:OrderedSig` slot admits — arrives on the value
channel's `Object` arm and binds into the child scope's `bindings.data` through the
ordinary copied-mode value door, like any other object value: a module is a value,
so there is no type-side parameter bind for it. A genuinely **type-denoting**
argument — a `:Type` slot, a bare type-name slot, `:Signature` — arrives on the
`Type` arm and [`run_user_fn`](../../src/machine/core/kfunction/exec.rs) writes it
into `bindings.types` via `register_type`; the argument arrives already resolved, so
the write is direct, with no per-call transient identity elaboration at the bind
site.

Body-position references to a module parameter (`Er.compare`, `Er.Carrier`) resolve
through the value channel: [`attr.rs`](../../src/builtins/attr.rs)'s
`body_identifier` finds the module in `data` and `body_module` projects the member
off the module value. A bare `Er` in type position (`-> Er`) lowers to the argument
module's self-sig — see
[modules.md § Module heads in type position](modules.md#module-heads-in-type-position).

FUNCTOR parameters are otherwise **unrestricted ordinary FN parameters**.
Because koan unifies the value and module languages — a module is a
first-class `KObject::Module` in the value channel's `Object` arm, a FUNCTOR an
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
`:Type`-typed parameter slot admits any type value in the `Type` arm — bare
builtin tokens (`Number`, `Str`, `Bool`, `Null`) and the
`KType::SetRef { .. }` a struct / union nominal token
resolves to — so `(MAKETREE Number)` against
`FUNCTOR (MAKETREE Elt :Type) -> …` binds `Elt = KType::Number` per call
with no call-site wrapping. The per-call type-side bind treats the
builtin-keyed and nominal-keyed paths identically: a body-position `Elt`
resolves to `KType::Number` through `Scope::resolve_type`, and a deferred
return like `-> :Elt` re-elaborates through the same dep-finish slot
check the nominal-keyed path uses. The wall stays in place on the other side: a
signature value routes through the `OfKind(Signature)` slot and a module *value*
through a `Signature { .. }` slot, neither of which a `:Type` slot admits — keeping
the `:Type` vs `:Module` overload distinction. OCaml structurally cannot match
this without modular implicits, because its module language is stratified
above the value language.

## Deferred return-type elaboration

Return-type expressions that reference a per-call FUNCTOR parameter
(`-> Er`, `-> Er.Type`, `-> :(Set WITH {Elt = Er})`)
ride a *deferred* return-type carrier through the per-call scope.
[`ExpressionSignature::return_type`](../../src/machine/model/types/signature.rs)
is a `ReturnType<'a>` enum, not a bare `KType`: `Resolved(KType)` covers
every static case (return types that don't reference a parameter), while
`Deferred(DeferredReturn<'a>)` holds the surface form verbatim — either
`TypeExpr(TypeName)` for parser-preserved leaf forms or
`Expression(KExpression<'a>)` for captured parens-form expressions. Routing
happens at binder construction in
[`fn_def.rs`](../../src/builtins/fn_def.rs): a parameter-name scan over the
captured return-type carrier picks `Deferred(_)` when any leaf matches a
parameter name and `Resolved(_)` otherwise. The parens-form overload
registers its return-type slot as `KType::KExpression` so the expression
survives binder definition without sub-dispatching against the outer scope.

Per-call elaboration runs in the body executor
[`run_user_fn`](../../src/machine/core/kfunction/exec.rs), which describes the
`Deferred(_)` outcome as a `Suspend { join, resume }`: `join` names the body
statements (plus, for the `Expression` carrier, the return-type expression as an
extra dep), and `resume` checks the body's terminal value once the deps resolve.
The dispatch-side [`invoke`](../../src/machine/execute/dispatch/exec.rs) is a
pure decide that lowers that `Suspend` into an `Outcome::ParkThenContinue` over
a single body-block [`DepRequest::BodyBlock`](../../src/machine/core/kfunction/action.rs) — the
body statements plus the return-type expression as deps in the harness-acquired
per-call frame (see
[per-call-region/frames.md § Active-frame propagation](../per-call-region/frames.md#active-frame-propagation))
— whose dep-finish runs `resume`. The `resume` closure
runs `per_call_ret.matches_value(body_value)` and surfaces mismatches with
`(per-call return type)` wording — a passing value is returned as-is (no
return-type stamp). The
`TypeExpr` carrier elaborates inline against the per-call scope, where the
parameter bind has installed the parameter-name identity — type-side for a
genuinely type-denoting argument, value-side for a module — and both carriers feed
the same dep-finish. The inline elaboration is the standard
[elaboration.md § Layers](elaboration.md#layers) § Layer 3 walk against
the per-call scope, run through
[`Scope::resolve_type_identifier`](../../src/machine/execute/dispatch/resolve_type_identifier.rs)
so the hit arrives with the *stored reach* of the binding it names. That reach is
what lets the resolved type be re-homed:
[`home_return_type`](../../src/machine/core/kfunction/exec.rs) moves it into the
captured-scope region (a live ancestor of the call), capped at the caller's
contract lifetime, and audits any region borrow it carries — a `Signature`'s
`decl_scope_ref`, or a bare module head's `SelfOf` module, which may live in some
*other* call's per-call region — against that reach. A region no evidence member,
no ambient coverage, and not the destination itself names is refused. The lift-time
return-type check in
[`run_loop.rs`](../../src/machine/execute/run_loop.rs)
gates on `ReturnType::is_resolved()` so the static-typing pathway stays
untouched and the deferred slot check runs only inside the dep-finish
finish where the per-call elaboration is in hand. The structural
`KType::KFunctor { ret }` synthesis at
[`function_value_ktype`](../../src/machine/model/values/kobject.rs) preserves the
deferred surface form structurally: a `Deferred(_)` source return projects into a
confined `KType::DeferredReturn(DeferredReturnSurface)` carrier holding the
deferred form's type-language shadow, rather than coarsening to `KType::Any`. The
admission helper at
[`function_compat`](../../src/machine/model/types/ktype_predicates.rs) then admits
a deferred return by syntactic shadow equality — an `Any` slot admits any deferred
return, a `KType::DeferredReturn` slot admits iff the shadows match
([ktype/parameterization-and-variance.md § Variance](ktype/parameterization-and-variance.md#variance)). The deferred-*parameter* half of this
precision — a per-call parameter type that reads as `Any` — is folded into
[modular implicits (stage 5)](../../roadmap/predicate_typing/modular-implicits.md),
where implicit search dispatches on parameter types.

Multi-argument FUNCTORs are ordinary multi-parameter binders. Currying is
just nested FUNCTORs whose outer return type is the inner functor's type,
written with the `:(FUNCTOR (params) -> R)` sigil:

```
LET MakeMap = (FUNCTOR (MAKEMAP Er :OrderedSig)
                -> :(FUNCTOR (Vo :MonoidSig) -> :(Map WITH {Key = Er.Type})) = (
  FUNCTOR (Vo :MonoidSig) -> :(Map WITH {Key = Er.Type}) = (
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
  (TYPE (Type AS Wrap))
  (VAL pure :(FN (x :Number) -> :(Number AS Wrap)))
  (VAL bind :(FN (m :(Number AS Wrap), f :(FN (x :Number) -> :(Number AS Wrap))) -> :(Number AS Wrap)))
)
```

`TYPE (<Param> AS <Name>)` is the declaration form (declaration-by-example:
it mirrors the application surface `:(Number AS Wrap)` with the concrete
argument replaced by the parameter name). Inside a SIG body it binds the
slot name (`Wrap` above) to a sentinel `KType::SetRef` whose member is a
`KKind::TypeConstructor` carrying the parameter symbol list. The declarator
lives in [`type_decl.rs`](../../src/builtins/type_decl.rs).

Application uses the `AS` keyworded builtin through the type-expression sigil:
`:(Number AS Wrap)` in a type-position slot lowers to
`KType::ConstructorApply { ctor: <the Wrap SetRef>, args: [Number] }` —
structural identity by `(ctor, args)`, mirror of `List(_)` / `Dict(_, _)`.
The constructor rides in as the `AS` right-hand `:Type` argument, not as a
dispatch verb, so the call routes through the ordinary keyworded path the
same way `:(LIST OF Number)` does; the
[`AS` builtin](../../src/builtins/parameterized_types.rs) checks the right-hand
side is a `TypeConstructor`-kind member and arity-checks against its
`param_names.len()`. A forward reference to an in-flight `LET` constructor
name parks on its producer through the same bare-name arg resolution every
`:Type` slot uses.

Higher-kinded slots are **per-call generative on the same path as ordinary
abstract type slots**. Two opaque ascriptions of the same source module
against the same SIG mint distinct `TypeConstructor` carriers under each
resulting module's `type_members[Wrap]` — they sit in distinct sets, so
their `(set ptr, index)` identities differ and `First.Wrap<Number>` and
`Second.Wrap<Number>` are incomparable types. The minting site is the same
loop in `ascribe.rs:body_opaque` that mints `KType::AbstractType` slots; it
inspects the SIG's `bindings.types[<slot>]` and matches a sentinel
`TypeConstructor`-kind member so the slot inherits its declared kind
(falling back to `AbstractType` for a plain `TYPE Type` slot).

The surface is **arity-1 only.** The `param_names` list always carries one
entry; multi-parameter constructors (`Functor F G`) are tracked in
[open-work.md](open-work.md). The parameter symbol must be a Type-classified
token (≥1 lowercase character): the parser rejects single-letter capitals
(`T`, `E`) at lex time, so surface forms in this section using `T` are
conceptual — real code writes `TYPE (Type AS Wrap)` or
`TYPE (Elt AS Wrap)`. The [token-class rule](tokens.md) is the
parser-level cause.

`ConstructorApply` is a type-language-only variant: no `KObject` reports a
`ConstructorApply` `ktype()`. The variant flows through the type-position
machinery (FN return-type elaboration, signature-body ascription) and the
value-level admissibility — wrapping a concrete value in `Wrap<Number>` and
unwrapping it — and cross-module application (`M.Wrap<Number>` reached via
ATTR-then-apply) are tracked in [open-work.md](open-work.md). A bare
`:(T AS Wrap)` in a signature body or against a root-scope-bound constructor
is the path the test suite pins.

## Type expressions and constraints

The `:(...)` type-expression sigil parameterizes `:(LIST OF T)`, `:(MAP K -> V)`,
and `:(FN (args) -> R)`
([ktype/parameterization-and-variance.md § Container type parameterization](ktype/parameterization-and-variance.md#container-type-parameterization))
for positional structural types. Sharing constraints,
modular-implicit signature constraints, and witness-typed
instantiations ride on the **infix `WITH` builtin**, which keys its
specializations by slot name in a record literal — `<sig> WITH {Slot = Type}`.
The two surfaces stay disjoint: `:(...)` for structural shapes whose
slot semantics are positional, `WITH {…}` for slot-named constraints.

- **`WITH`.** Infix signature specialization — `<sig> WITH {Slot = Type, …}`
  pins abstract type slots of a signature to specific concrete types.
  `(OrderedSig WITH {Type = Number})` is `OrderedSig` with its `Type` slot
  pinned to `Number`; `(Set WITH {Elt = Number, Ord = IntOrd})` pins multiple
  slots in one call. The bindings are a record literal keyed by slot name
  (capitalized Type-token field names); each `Slot = Type` field is one pin.
- **Type-valued slot values.** `WITH` slot values accept any
  expression that evaluates to a `KType`, not only bare type-name
  tokens. `(MySig WITH {Elt = Mo.Type})`
  works because the dotted `Mo.Type` access returns the abstract type of
  module `Mo`. The slot's declared kind decides what the engine expects.
- **Module-kind slots.** Type constructors can declare slots that take
  modules. `(Set WITH {Elt = Number, Ord = IntOrd})` works because
  `Set`'s `Ord` slot is declared `OrderedSig`-kind. Distinct module
  values bound to the same slot give distinct concrete types — the
  mechanism behind witness types in
  [open-work.md](open-work.md).

Sharing constraints, modular-implicit signature constraints, and
witness-typed instantiations share this one builtin family. The
implicit *marker* itself (which parameter is implicit) is orthogonal —
see [implicits.md](implicits.md).
