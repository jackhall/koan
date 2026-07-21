# Functors

A **functor** — a module-returning function — is how koan parameterizes a module by another
module. It is not a construct: it is an ordinary `FN` whose body evaluates to a module value,
so a functor is a *special case of a function*, named for the pattern rather than for any
machinery. `FN` is koan's only function binder. A functor may also take a bare `:Type`
parameter; generic functions are built this way — see [generics.md](generics.md).

- *Surface semantics* — a functor's module parameter is an ordinary **value**
  parameter under a signature-typed slot (`er :Ordered`), so its name is
  Identifier-class (`er`, `mo` — lowercase-first per [tokens.md](tokens.md)). The
  parameter still reaches type position, through two projections: the dotted
  `er.Carrier`, which extracts the module's abstract type member, and
  `:(TYPE OF er)`, which names the argument module's own principal signature (see
  [modules.md § Modules in type position](modules.md#modules-in-type-position-type-of)).
  A bare `er` in a slot or a return is an error — a value token names no type.
- *Machine semantics* — modules are **first-class values**.
  `KObject::Module(&Module)` flows through the scheduler in the value channel's `Object` arm
  like any other value (a signature, by contrast, rides the
  [`Carried::Type`](../../src/machine/model/values/carried.rs) arm alongside `Number`, `Str`,
  and other type values). A functor is internally an ordinary `KFunctionValue`: same
  dispatch path, same scheduler integration, same per-call scope, same body executor
  (`run_user_fn`), and its `ktype()` is `KType::KFunction`. The engine holds no
  functor-specific state — no flag, no type variant, no binder.

```
LET make_set = (FN (MAKESET er :Ordered) -> Module = (
  MODULE result = (
    (LET Carrier = ...)
    (LET insert = (FN (INSERT s :Carrier x :er.Carrier) -> Carrier = ...))
    ...
  )
))

LET int_set = (MAKESET int_ord)
```

`MODULE <name> = (...)` is itself an expression: it both binds the name in the
enclosing per-call scope and evaluates to the module value, so the FN body needs
no separate "anonymous structure" form. The bound name (`result` above) lives only
inside the call frame.

An FN's return slot carries no module-specific obligation: an FN may return anything, and a
module return is checked like any other by the ordinary per-call return contract. Whether a
given FN is "a functor" is a reading of its return slot, not a property the engine stores.

## Application and binding

A functor binds and applies exactly like any other function.

The binder registers the keyword overload in the dispatch table, so `(MAKESET int_ord)` — the
ordinary keyworded call convention — is the primary application surface. A
`LET make_set = (FN …)` additionally binds the function *value* under a snake_case
(value-class) name in `bindings.data`, reachable through the value-side function-value call
and its one-record-literal named-args form, `(make_set {er = int_ord})`. `bindings.types`
holds no callable value: binding a function under a Type-class (capitalized) name is a
`TypeClassBindingExpectsType` error at the LET site (see
[elaboration.md § Binding-map partition](elaboration.md#binding-map-partition)), and there is
no Type-head application surface for a function. Both call surfaces route through
`apply_callable`'s `Function` arm — the same arm a plain function call takes — and the result
happens to be a module.

Classification of a head into a callable is owned by
[execution/name-placeholders.md § Dispatch-time name placeholders](../execution/name-placeholders.md#dispatch-time-name-placeholders);
a functor needs no arm of its own there either.

## Type identity

A functor's type is `KType::KFunction { params, ret }` — the same structural variant every
function reports. `params` is a name-keyed
[parameter `Record<KType>`](ktype/records-and-limits.md#record-fields-and-ktype-hashing), so a
functor's parameter names (including capitalized `Type`-token names like `Ty` for a `:Type`
parameter) are part of its identity and round-trip through `KType::name()`; identity is the
record's order-blind equality. Admissibility is the ordinary function-subtyping rule at
[`function_compat`](../../src/machine/model/types/ktype_predicates.rs) — contravariant params
with width-drop, covariant return (see
[ktype/parameterization-and-variance.md § Variance](ktype/parameterization-and-variance.md#variance))
— and `KType::join` joins two same-shape functions to a shared `KFunction`.

A module-returning function is therefore admissible wherever a same-shape `:(FN …)` slot
matches, and joins with plain functions: there is no type-level partition between "returns a
module" and "returns anything else". The only function-type surface is `:(FN (params) -> R)`.

## Generativity

Functor application is **generative**: each call evaluates the body afresh,
and any inner `:|` mints fresh `KType::AbstractType { source_module, name }`
slots. `(MAKESET int_ord)` applied twice yields two distinct `Set` types
that cannot be confused. Generativity is a consequence of `:|`-per-call;
the mechanism is general — any FN that contains `:|` mints fresh slots on
each call.

An **applicative** variant — same-functor-applied-to-same-module producing
the same output types, so independent call sites resolving to the same
implicit module interoperate — is deferred behind the predicate-typing
work. The language stays generative-only until that substrate lands. The seam
applicative mode keys on is a *derived* classification of the return slot — "does this return
slot name a signature?" — computed on demand from the slot rather than stored, since a module
cannot be named in type position and so a module-returning function's return slot always names
a signature. See [open-work.md](open-work.md).

## Sharing constraints

Sharing constraints — pinning a functor's output abstract type to a
specific concrete type — ride on the `WITH` builtin described in
[Type expressions and constraints](#type-expressions-and-constraints). An FN
whose return type is `:(Set WITH {Elt = Number})` declares
the constraint at the return slot; the body's `MODULE result` must mirror
`Elt = Number` for the per-call return check to admit it. There is no separate
`with type` keyword.

Pin values that reference only the FN's outer scope are elaborated at
binder-construction time. Concrete builtins (`Number`, `Str`) and
outer-scope-bound type values (`mo.Carrier` where `mo` is
bound outside the FN) both work as pin values resolved eagerly.

## Parameters

An FN parameter binds per-call into the universe **its own name** picks — not whichever
channel its argument happens to travel
([tokens.md § Token class is a binding rule](tokens.md#token-class-is-a-binding-rule-not-just-a-lexical-one)).
A **module**-valued parameter is named snake_case (`er :Ordered`); its argument arrives on the
value channel's `Object` arm and binds into the child scope's `bindings.data` through the
ordinary copied-mode value door, like any other object value: a module is a value, so there is
no type-side parameter bind for it. A **type-denoting** parameter — a `:Type` slot, a bare
type-name slot, `:Signature` — is named with a Type token (`Ty`, `Er`); its argument arrives on
the `Type` arm and [`run_user_fn`](../../src/machine/core/kfunction/exec.rs) writes it into
`bindings.types` via `register_type`; the argument arrives already resolved, so the write is
direct, with no per-call transient identity elaboration at the bind site. Mixing the two —
handing a module to a Type-token parameter, or a type to a snake_case one — is refused by the
binding maps' partition guard at the bind.

Body-position references to a module parameter (`er.compare`, `er.Carrier`) resolve
through the value channel: [`attr.rs`](../../src/builtins/attr.rs)'s
`body_identifier` finds the module in `data` and `body_module` projects the member
off the module value. The argument module's own signature is named `:(TYPE OF er)` —
see [modules.md § Modules in type position](modules.md#modules-in-type-position-type-of).

A functor's parameters are **unrestricted ordinary FN parameters** — there is no other kind.
Because koan unifies the value and module languages — a module is a first-class
`KObject::Module` in the value channel's `Object` arm — a module-returning FN can take
anything an FN can take, including a bare value
(`FN (MAKETREE factor :Number) -> …`, with
the body's `MODULE` closing over `factor` lexically). This is where koan
departs from OCaml: OCaml stratifies a separate module language above the
value language, so a functor takes only module arguments and a value must be
smuggled in via `struct let factor = 4 end`. Koan has no such stratum, so a
value parameter binds directly — no wrapping, and no requirement that any
parameter be signature-typed. A value passed this way is **runtime
data, not part of type identity**: per-call generativity still mints fresh
abstract types each call, but two calls differing only in the value produce
structurally identical type members. Koan has no type-level values, so a
value parameter never enters type identity; const-generic-style
parameterization, where the value *is* part of the type, is a different
model koan does not adopt.

The same no-stratum reasoning extends symmetrically to bare type tokens. A
`:Type`-typed parameter slot admits any type value in the `Type` arm — bare
builtin tokens (`Number`, `Str`, `Bool`, `Null`) and the
`SetMember` handle a struct / union nominal token
resolves to — so `(MAKETREE Number)` against
`FN (MAKETREE Elt :Type) -> …` binds `Elt = KType::NUMBER` per call
with no call-site wrapping. The per-call type-side bind treats the
builtin-keyed and nominal-keyed paths identically: a body-position `Elt`
resolves to `KType::Number` through `Scope::resolve_type`, and a deferred
return like `-> :Elt` re-elaborates through the same dep-finish slot
check the nominal-keyed path uses. The distinction between slots stays in place on the other
side: a signature value routes through the `OfKind(Signature)` slot and a module *value*
through a `Signature { .. }` slot, neither of which a `:Type` slot admits — keeping
the `:Type` vs `:Module` overload distinction. OCaml structurally cannot match
this without modular implicits, because its module language is stratified
above the value language.

## Deferred return-type elaboration

Return-type expressions that reference a per-call parameter
(`-> :(TYPE OF er)`, `-> er.Carrier`, `-> :(Set WITH {Elt = er.Carrier})`)
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
so the hit arrives as a bare region `&KType`. Re-homing it needs no residence
evidence: [`home_return_type`](../../src/machine/core/kfunction/exec.rs) clones the
owned type into the captured-scope region (a live ancestor of the call) through the
single type door, capped at the caller's contract lifetime so the `ret` reference
cannot outlive the window the lift boundary consumes it in. The cap is
return-contract discipline, not a residence audit — a `KType` has no residence to
audit. The lift-time
return-type check in
[`run_loop.rs`](../../src/machine/execute/run_loop.rs)
gates on `ReturnType::is_resolved()` so the static-typing pathway stays
untouched and the deferred slot check runs only inside the dep-finish
finish where the per-call elaboration is in hand. The structural
`KType::KFunction { ret }` synthesis at
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

Multi-argument functors are ordinary multi-parameter FNs. Currying is
just nested FNs whose outer return type is the inner function's type,
written with the `:(FN (params) -> R)` sigil:

```
LET make_map = (FN (MAKEMAP er :Ordered)
                 -> :(FN (vo :Monoid) -> :(Map WITH {Key = er.Carrier})) = (
  FN (MAKEVALS vo :Monoid) -> :(Map WITH {Key = er.Carrier}) = (
    MODULE result = ( ... )
  )
))
```

The inner FN inherits the outer's per-call scope, so `er.Carrier` in its return slot resolves
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

`TYPE (<Param>… AS <Name>)` is the declaration form (declaration-by-example:
it mirrors the application surface with the concrete arguments replaced by
the parameter names). Inside a SIG body it binds the slot name (`Wrap`
above) to a `KType::AbstractType` whose `param_names` are the declared
parameters — the same variant a first-order `TYPE Carrier` binds, which
carries an empty list. One or more parameters may be declared, and a
repeated name in one declaration is a shape error, since the names key the
application record. `KType::kind_of` reads the list: empty is
`KKind::ProperType`, non-empty `KKind::TypeConstructor`. The declarator
lives in [`type_decl.rs`](../../src/builtins/type_decl.rs). The value-level
counterpart `NEWTYPE (Elem AS Wrap)` declares a constructor family a module
can supply as the witness for this slot, and constructs values inhabiting
the applied type — see
[user-types.md § Constructor families](user-types.md#constructor-families-newtype-type-as-wrapper).

**Parameter names are interface.** They feed the SIG schema's content digest
(sorted, so declaration order is presentation), and satisfaction requires
name agreement: a module supplying `NEWTYPE (Item AS Wrap)` does *not* fill
a `TYPE (Elem AS Wrap)` slot. The mismatch surfaces as
`SigSubtypeFailure::KindMismatch`, naming the parameter set the slot
declares. Renaming a type parameter is therefore a breaking change to
satisfaction, exactly as renaming a `KFunction` parameter is.

Application binds each parameter by name, through a record literal on the
constructor identity: `:(Wrap {Elem = Number})`, `:(Result {Ok = Number,
Error = MyError})`. It lowers to
`KType::ConstructorApply { ctor: <the Wrap identity>, args: {Elem = Number} }` —
structural identity by `(ctor, args)`, with `Record`'s order-blind
identity, so the same name-to-type map is the same application however it
was written. The arm lives in
[`apply_callable.rs`](../../src/machine/execute/dispatch/apply_callable.rs):
a constructor-kind head with a record-literal body launches one sub-dispatch
per field, so a compound argument (`{Elem = (LIST OF Number)}`) elaborates
through the ordinary type-expression lanes and the slot parks until it
lands. The supplied key set must equal the parameter set; a mismatch names
the missing and unknown keys. The args are stored in the constructor's
declared order, which is what `KType::name()` renders and re-parses.

`AS` survives as **arity-1 sugar**: `:(Number AS Wrap)` fills the
constructor's sole parameter by that parameter's own name, so it elaborates
to a type digest-equal to `:(Wrap {Elem = Number})`. The constructor rides
in as the `AS` right-hand `:Type` argument, not as a dispatch verb, so the
call routes through the ordinary keyworded path the same way
`:(LIST OF Number)` does; the
[`AS` builtin](../../src/builtins/parameterized_types.rs) reads the
right-hand side's parameter names and errors when there is not exactly one,
directing to the record form. A forward reference to an in-flight `LET`
constructor name parks on its producer through the same bare-name arg
resolution every `:Type` slot uses.

Higher-kinded slots are **per-call generative on the same path as ordinary
abstract type slots**. Two opaque ascriptions of the same source module
against the same SIG mint distinct `TypeConstructor` carriers under each
resulting module's `type_members[Wrap]` — they sit in distinct sets, so
their `(set ptr, index)` identities differ and `First.Wrap` and
`Second.Wrap` applied at `Number` are incomparable types. The minting site
is the same loop in `ascribe.rs:body_opaque` that mints
`KType::AbstractType` slots; it inspects the SIG's
`bindings.types[<slot>]` and mints a fresh constructor family over the
slot's own `param_names` whenever that list is non-empty (falling back to a
fresh first-order `AbstractType` for a plain `TYPE Carrier` slot).

The parameter symbol must be a Type-classified token (≥1 lowercase
character): the parser rejects single-letter capitals (`T`, `E`) at lex
time, so real code writes `TYPE (Type AS Wrap)` or `TYPE (Elem AS Wrap)`.
The [token-class rule](tokens.md) is the parser-level cause, and it is why
the builtin `Result` names its parameters `Ok` and `Error` rather than `T`
and `E`.

`ConstructorApply` flows through the type-position machinery (FN return-type
elaboration, signature-body ascription) and also names a **runtime value's**
type: a value constructed over a `NEWTYPE (Type AS Wrapper)`-declared family
reports a `ConstructorApply` `ktype()`, so wrapping a concrete value in
`Wrapper (v)` and dispatching on `:(Number AS Wrapper)` both ship — see
[user-types.md § Constructor families](user-types.md#constructor-families-newtype-type-as-wrapper).
*Value* construction stays an arity-1 surface: an identity wrapper wraps one
value and infers one type argument from it, so constructing a value of a
family declaring two or more parameters is a shape error naming the arity.
The *type* surface has no such limit — a multi-parameter family applies by
name like any other. Still future and tracked in
[open-work.md](open-work.md): re-tagging an applied-constructor-typed VAL
slot read through an opaque view, and cross-module application
(`:(Number AS mo.Wrap)` over another module's constructor member, reached
via ATTR-then-apply). A bare `:(Number AS Wrap)` in a signature body or
against a root-scope-bound constructor is the path the test suite pins.

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
  `(Ordered WITH {Carrier = Number})` is `Ordered` with its `Carrier` slot
  pinned to `Number`; `(Set WITH {Elt = Number, Ord = :(TYPE OF int_ord)})` pins multiple
  slots in one call. The bindings are a record literal keyed by slot name
  (capitalized Type-token field names); each `Slot = Type` field is one pin.
- **Type-valued slot values.** `WITH` slot values accept any
  expression that evaluates to a `KType`, not only bare type-name
  tokens. `(Pinnable WITH {Elt = mo.Carrier})`
  works because the dotted `mo.Carrier` access returns the abstract type of
  module `mo`. The slot's declared kind decides what the engine expects.
- **Module-kind slots.** Type constructors can declare slots that take
  modules. `(Set WITH {Elt = Number, Ord = :(TYPE OF int_ord)})` works because
  `Set`'s `Ord` slot is declared `Ordered`-kind. Distinct module
  values bound to the same slot give distinct concrete types — the
  mechanism behind witness types in
  [open-work.md](open-work.md).

Sharing constraints, modular-implicit signature constraints, and
witness-typed instantiations share this one builtin family. The
implicit *marker* itself (which parameter is implicit) is orthogonal —
see [implicits.md](implicits.md).
