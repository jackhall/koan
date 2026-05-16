# Functors

A **functor** is a module parameterized by another module — a function from
modules to modules. Koan presents this with two layered semantics:

- *Surface semantics* — modules are part of the **type language**. A
  signature-typed FN parameter (`Er: OrderedSig`) is a type-language
  binder, like an OCaml functor's parameter. `Er.Type` in a type-position
  slot is type-language projection — extracting the module's abstract
  type. Identifier-class names (`er`, `mo` — lowercase-first per
  [tokens.md](tokens.md)) are value-language only and a hard error in any
  type-position slot.
- *Machine semantics* — modules are **first-class values**.
  `KObject::KModule` flows through the scheduler like any other value;
  functors are ordinary FNs whose parameters are signature-typed and whose
  body returns a `MODULE` expression.

The two readings rest on the same scheduler — there is no separate
type-checking pass, no parallel module language. The elaborator's
token-class-driven lookup is the seam: Type-class names in type-position
slots consult the type-language binders; identifier-class names do not.
The example below illustrates both readings — the surface reads `Er` as a
type-language binder, the machine sees a value parameter whose value is a
module:

```
LET MakeSet = (FN (MAKESET Er :OrderedSig) -> SetSig = (
  MODULE Result = (
    (LET Type = ...)
    (LET insert = (FN (INSERT s :Type x :Er.Type) -> Type = ...))
    ...
  )
))

LET IntSet = (MAKESET IntOrd)
```

`MODULE Name = (...)` is itself an expression: it both binds `Name` in the
enclosing per-call scope and evaluates to the module value, so the functor
body needs no separate "anonymous structure" form. The bound name (`Result`
above) lives only inside the call frame.

Functor application is **generative**: each call evaluates the body afresh,
and any inner `:|` mints fresh `KType::UserType { kind: Module, .. }`
slots. `(MAKESET IntOrd)` applied twice yields two distinct `Set` types
that cannot be confused.
Generativity is a consequence of `:|`-per-call, not a separate mechanism.
The applicative variant — same-functor-applied-to-same-module producing the
same output types, so independent call sites resolving to the same implicit
module interoperate — is tracked in [open-work.md](open-work.md).

Sharing constraints — pinning a functor's output abstract type to a
specific concrete type — ride on the `SIG_WITH` builtin described in
[Type expressions and constraints](#type-expressions-and-constraints). A
functor whose return type is `(SIG_WITH SetSig ((Elt: Number)))` declares
the constraint at the FN's return slot; the body's `MODULE Result`
must mirror `Elt = Number` for the return-type check to admit it. There
is no separate `with type` keyword.

Pin values that reference only the FN's outer scope are elaborated at
FN-construction time. Concrete builtins (`Number`, `Str`) and
outer-scope-bound type values (`(MODULE_TYPE_OF Mo Type)` where `Mo` is
bound outside the FN) both work as pin values resolved eagerly.

A Type-class FN parameter (`Er: OrderedSig`) binds the parameter name as
a type-language binder at the call site: at each call,
[`KFunction::invoke`](../../src/machine/core/kfunction/invoke.rs)
dual-writes the per-call argument into the child scope's `bindings.types`
alongside the existing value-side `bind_value`. The
[`KType::is_type_denoting`](../../src/machine/model/types/ktype_predicates.rs)
predicate gates the dual-write — `SignatureBound`, `Signature`, `Type`,
`TypeExprRef`, and `AnyUserType { kind: Module }` carry meaningful
type-language identity at the binder. Body-position references to the
parameter (`(MODULE_TYPE_OF Er Type)` inside the body) resolve through
`Scope::resolve_type`'s outer-chain walk against the per-call scope.

Return-type expressions that reference a per-call FN parameter
(`-> Er`, `-> (MODULE_TYPE_OF Er Type)`, `-> (SIG_WITH Set ((Elt: Er)))`)
ride the same per-call scope through a *deferred* return-type carrier.
[`ExpressionSignature::return_type`](../../src/machine/model/types/signature.rs)
is a `ReturnType<'a>` enum, not a bare `KType`: `Resolved(KType)` covers
every static case (builtins and FNs whose return type doesn't reference a
parameter), while `Deferred(DeferredReturn<'a>)` holds the surface form
verbatim — either `TypeExpr(TypeExpr)` for parser-preserved structured
forms or `Expression(KExpression<'a>)` for captured parens-form
expressions. Routing happens at FN-definition in
[`fn_def.rs`](../../src/builtins/fn_def.rs): a parameter-name scan
over the captured return-type carrier picks `Deferred(_)` when any leaf
matches a parameter name and `Resolved(_)` otherwise. The parens-form
overload registers its return-type slot as `KType::KExpression` so the
expression survives FN-def without sub-dispatching against the outer
scope.

Per-call elaboration runs at the dispatch boundary in
[`KFunction::invoke`](../../src/machine/core/kfunction/invoke.rs). The
`Deferred(_)` arm spawns the body Dispatch and (for the `Expression`
carrier) an optional return-type sub-Dispatch under the per-call frame
via `SchedulerHandle::with_active_frame`, then joins them in a `Combine`
whose finish closure runs `per_call_ret.matches_value(body_value)` and
surfaces mismatches with `(per-call return type)` wording. The
`TypeExpr` carrier elaborates inline against the per-call scope where the
dual-write has installed the parameter-name identities; both carriers feed
the same Combine. The lift-time return-type check in
[`scheduler/execute.rs`](../../src/machine/execute/scheduler/execute.rs)
gates on `ReturnType::is_resolved()` so the static-typing pathway stays
untouched and the deferred slot check runs only inside the Combine
finish where the per-call elaboration is in hand. The structural
`KType::KFunction { ret }` synthesis at
[`function_value_ktype`](../../src/machine/model/values/kobject.rs) and the
admission helper at
[`function_compat`](../../src/machine/model/types/ktype_predicates.rs)
coarsen `Deferred(_)` to `KType::Any` because the structural function-type
language has no surface for "per-call elaboration of this expression" —
see [open-work.md](open-work.md) for the precision refinement.

Multi-argument functors are ordinary multi-parameter FNs. Currying is just
nested FNs.

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
mints `kind: Module` slots; it inspects the SIG's
`bindings.types[<slot>]` and matches `UserTypeKind::TypeConstructor` so the
slot inherits its declared kind.

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
  expression that evaluates to a `KType` or `KModule`, not only bare
  type-name tokens. `(SIG_WITH MySig ((Elt: (MODULE_TYPE_OF Mo Type))))`
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
