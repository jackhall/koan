# `KType` — the runtime type system

[`KType`](../../src/machine/model/types/ktype.rs) has a variant for every concrete `KObject`:

- Scalars: `Number`, `Str`, `Bool`, `Null`.
- Containers: `List(Box<KType>)`, `Dict(Box<KType>, Box<KType>)`,
  `KFunction { args: Vec<KType>, ret: Box<KType> }`. Always parameterized; see
  [Container type parameterization](#container-type-parameterization) below.
- Other function-like: `KExpression` (a captured-but-unevaluated expression).
- Meta-type for type-position slots: `TypeExprRef` — see
  [Type-position slot kinds](#type-position-slot-kinds).
- First-class type values: `Type` (a tagged-union or struct schema, the meta-type
  reported by `KObject::StructType` and `KObject::TaggedUnionType`). As a
  parameter-slot annotation (`:Type`), it admits any type-denoting carrier:
  bare builtin type tokens (`Number`, `Str`, `Bool`, `Null`) carried as
  `KObject::KTypeValue(_)`, tagged-union and struct schema carriers, and any
  other non-module / non-signature `KTypeValue`. Module and signature
  carriers route through the dedicated `AnyModule` / `AnySignature` /
  `SatisfiesSignature` slots so the `:Type` vs `:Module` overload
  distinction stays intact — see
  [`KType::Type::accepts_part`](../../src/machine/model/types/ktype_predicates.rs)
  and the pin test
  [`type_slot_admits_bare_builtin_tokens_and_user_type_carriers`](../../src/machine/model/types/ktype_predicates/tests.rs).
- User-declared nominal types: `UserType { kind: UserTypeKind, scope_id: usize,
  name: String }` — the per-declaration identity tag synthesized by
  `KObject::ktype()` for `Struct` and `Tagged` carriers. Two distinct STRUCTs
  produce different `scope_id`s, giving the per-declaration-distinctness
  identity property dispatch keys on.
  `UserTypeKind` is `Struct | Tagged | Newtype { repr } |
  TypeConstructor { param_names }`. The two payload-carrying variants
  (`Newtype`, `TypeConstructor`) have a manual `PartialEq` that ignores their
  payloads — identity equality is by variant tag plus the carrier's
  `(scope_id, name)`, so wildcard / concrete pairs compare equal.
  The companion `AnyUserType { kind }` wildcard accepts any `UserType` of the
  matching kind, used for slot types that admit "any user-declared X" — ATTR's
  `body_struct` slot, construction primitives' return types. The surface
  keywords `Newtype` and `TypeConstructor` are pinned for diagnostic rendering
  but not registered as writable surface names (no entry in
  [`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs)).
- Module / signature carriers (the [module system](modules.md) rests on
  these): `Module { module: &'a Module<'a>, frame: Option<Rc<CallArena>> }`
  is the first-class module value's type — the arena-pinned `&Module`
  pointer plus the per-call frame anchor for functor-built modules;
  `Signature(&'a Signature<'a>)` is the first-class signature value's
  type; `AbstractType { source_module: &'a Module<'a>, name: String }`
  is the per-abstract-type-member tag minted by `:|` opaque ascription.
  Manual `PartialEq` keys identity on `module.scope_id()` for
  `KType::Module`, `s.sig_id()` for `KType::Signature`, and
  `(source_module.scope_id(), name)` for `KType::AbstractType` — so two
  opaque ascriptions of the same source module produce distinct
  `KType::Module` identities (the abstraction barrier) but their
  `AbstractType` minting for the same slot name compares equal.
  Companion wildcards `AnyModule` and `AnySignature` admit any module
  or signature value respectively; the surface keywords `Module` and
  `Signature` lower to them in
  [`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs).
  `SatisfiesSignature { sig_id, sig_path, pinned_slots }` is the
  slot-annotation form ("any module satisfying this signature"): it's
  what `Er :OrderedSig` lowers to in a FUNCTOR parameter slot. The
  identity-bearing `Signature(_)` variant carries the value, while
  `SatisfiesSignature` constrains a slot — both reach the same
  `sig_id()` for the dispatch key.
- Higher-kinded application: `ConstructorApply { ctor: Box<KType>, args:
  Vec<KType> }` — structural identity by `(ctor, args)`, mirror of `List(_)`
  / `Dict(_, _)`. Emitted by `elaborate_type_expr` when the outer name of a
  parameterized `TypeExpr` resolves to a
  `UserType { kind: TypeConstructor { .. }, .. }`; renders as `ctor<arg1,
  arg2>` in diagnostics. See
  [functors.md § Higher-kinded type slots](functors.md#higher-kinded-type-slots)
  for the surface form and per-call generativity.
- `Any` — the no-op fast-path.

[`KType::matches_value`](../../src/machine/model/types/ktype_predicates.rs) plus
[`KObject::ktype`](../../src/machine/model/values/kobject.rs) close the loop on runtime
checking: every value has a queryable type, and any declared type can be checked
against it.

## Container type parameterization

`:(List T)`, `:(Dict K V)`, and `:(Function (args) -> ret)` carry their inner
types on the variant directly. `KType` is not `Copy`; structural payloads are
`Box`ed where the variant would otherwise be self-referential.

**Surface syntax** is a glued-right `:` sigil opening an S-expression
type-expression group. The parser treats `:(...)` as a type-position frame
anchored to the `:` — `:(List Number)` is one
[`ExpressionPart::Type`](../../src/machine/model/ast.rs) carrying a structured
`TypeExpr`, not four tokens. `<` and `>` flow through unencumbered as
keyword tokens, leaving the arithmetic comparison operators available. The
framing logic lives in
[type_expr_frame.rs](../../src/parse/type_expr_frame.rs).

### Variance

Variance is split across the parameterized constructors. `List` and `Dict` are
covariant in their parameter positions; `Function` is invariant in args and
return. The split falls out of the underlying check in each case rather than
being a deliberate design dial — both choices are the natural one given how
the constructor's values are matched, and the conservative `Function`-invariant
rule keeps dispatch unambiguous.

Three sites consume parameterized types, and each has its own behavior:

| Site | What it does | Variance |
| --- | --- | --- |
| `matches_value` | Walks a runtime value against a declared type at an ascription boundary (FN return, FN argument, `LET`). | **Covariant** for `List` / `Dict`: `:(List Any)` accepts any list because `Any.matches_value(_)` is always true; `:(Dict Str Any)` accepts a `{a: 1, b: "x"}` value. **Invariant** for `Function`: delegates to `function_compat`. |
| `is_more_specific_than` | Ranks two slot types when multiple overloads match the same call. Used by `specificity_vs` to break dispatch ties. | **Covariant in every parameter position** (element, key, value, arg, ret): `:(List Number)` ≺ `:(List Any)`, `:(Dict Str Number)` ≺ `:(Dict Str Any)`, `:(Function (Number) -> Str)` ≺ `:(Function (Any) -> Any)`. |
| `function_compat` | The dispatch-time check that a `KObject::KFunction` value fills a typed function-shaped slot. | **Strict structural equality** — invariant. A function declared `(x :Number) -> Str` fills only `:(Function (Number) -> Str)`, not `:(Function (Any) -> Str)`. |

The combination is sound for dispatch even though `is_more_specific_than`
ranks `Function`-typed slots covariantly while `function_compat` is invariant.
The covariant ranking only matters when two parameterized function slots both
match the same call; with `function_compat`'s strict equality, a function
value matches at most one parameterized function slot, so the ranking has no
tie to break in that case. The covariance is observable for `List` / `Dict`
tournaments — `(xs :(List Number))` strictly outranks `(xs :(List Any))` for a
number-list call — and benign for `Function`.

Concretely:

```
LET nums = [1 2 3]

FN (PICK xs :(List Any))    -> Str = ("any")
FN (PICK xs :(List Number)) -> Str = ("number")

PICK nums   # → "number"   (covariant: :(List Number) ≺ :(List Any))
```

```
FN (BAD) -> :(List Number) = ([1 "x"])
BAD   # → TypeMismatch: expected :(List Number), got :(List Any)
        # (matches_value walks elements; covariant — Any.matches_value(_) is true,
        #  Number.matches_value("x") is false)
```

```
FN (USE f :(Function (Number) -> Str)) -> Str = ("got fn")

USE (FN (SHOW x :Number) -> Str = ("hi"))   # → "got fn"   (function_compat: equal)
USE (FN (SHOW x :Any)    -> Str = ("hi"))   # → DispatchFailed
                                            #   (function_compat: invariant, not equal)
```

**Element-type inference for literals** is the join of element types via
[`KType::join_iter`](../../src/machine/model/types/ktype_resolution.rs), computed
**once at construction** and memoized on the value's carrier: `[1, 2, 3]` →
`List<Number>`, `[1, "x"]` → `List<Any>`. `KObject::List` and `KObject::Dict`
each carry their element types directly (`List(Rc<Vec<…>>, Box<KType>)`,
`Dict(…, Box<KType>, Box<KType>)`), so
[`KObject::ktype`](../../src/machine/model/values/kobject.rs) reads the carried
type in O(1) rather than re-walking the contents on every call. Values are
immutable `Rc`, so the join is sound to compute exactly once. Functions project
their declared signature (`KObject::KFunction(f, _)` → `KFunction { args, ret }`
read off `f.signature`).

**Empty containers carry no element type to infer**, so an unstamped empty `[]`
/ `{}` (element type memoized as `Any`, never stamped by an annotation) is an
**error** at an untyped resolution boundary — an untyped value-route `LET`, a
bare top-level expression result. The producing boundary must annotate the value
(e.g. a typed FN return) or use a non-empty literal. A *stamped* empty container
(an `FN -> :(List Number) = ([])` whose carrier is re-tagged to element `Number`)
is fine; a heterogeneous non-empty literal (`[2, "hello"]` → `List<Any>`) is
unaffected — it carries information and is legal where `:(List Any)` is declared.

### Runtime type-parameter carriers

`List`, `Dict`, and `Tagged` carry their runtime type arguments on the variant so
dispatch and slot admission see the full instantiation, not just the outer shape:

- `KObject::List(items, elem)` / `KObject::Dict(map, key, value)` memoize the
  element / key+value type at construction (`KObject::list` / `KObject::dict`).
- `KObject::Tagged { type_args, .. }` carries the applied type arguments of a
  parameterized union (`Result<T, E>`). Empty `type_args` means erased — `ktype()`
  reports the bare `UserType` as before; a populated carrier makes `ktype()`
  synthesize `ConstructorApply { ctor, args: type_args }`. Construction
  (`tagged_union::construct`, `CATCH`) erases by default; the carrier is populated
  only by ascription stamping.

A `ConstructorApply` slot (`:(Result T E)`) admits a `Tagged` value via the
`matches_value` arm in
[ktype_predicates.rs](../../src/machine/model/types/ktype_predicates.rs): the
declaring schema must be the same constructor, and then either the populated
`type_args` are checked structurally against the declared args, or — for an erased
carrier — the *inhabited* tag's payload is checked against the type argument that
field maps to. The `Result` field→parameter linkage (`ok`→0 / `T`, `error`→1 /
`E`) lives in the type layer as `result_field_param_index`, reading the ordering
the builtin registration owns.

**Ascription is authoritative at annotated boundaries.** A parameterized-carrier
value crossing an annotated boundary is checked via `matches_value` and then
re-tagged (`KObject::stamp_type`) to *exactly* the declared type, **coarsening
included** — a `List<Number>` value returned through `:(List Any)` re-tags to
`List<Any>`, so downstream dispatch sees the contract rather than the
implementation's incidental precision. An unannotated value keeps its precise
memoized type; surrendering precision is the deliberate act of writing an
annotation. The three boundaries are:

- **FN return** — the scheduler walks `matches_value` over the returned value
  (a list literal `[1, "x"]` returned where `:(List Number)` was declared fails
  with a structured `TypeMismatch` naming both types), then stamps the carrier to
  the resolved per-call return type. Both the resolved and deferred-return paths
  stamp in [`invoke.rs`](../../src/machine/core/kfunction/invoke.rs).
- **FN argument** — the invoke bind loop runs `matches_value` on each evaluated
  parameterized-carrier argument slot (`List` / `Dict` / `ConstructorApply`),
  then coarsens via `stamp_type`. `bundle.args` holds evaluated values at this
  point (only `KExpression` slots stay lazy by design), so the bind loop is a
  valid value boundary symmetric with the return check. This `matches_value` walk
  is the authoritative content-recursive check; for `List` / `Dict` it confirms
  what dispatch already gates, since an evaluated container whose carried element
  type doesn't satisfy the slot is rejected as a dispatch non-match (see
  [Dispatch and slot-specificity](#dispatch-and-slot-specificity)).
- **`LET`** ascription — same check-then-stamp on the bound value.

**Arity is enforced at FN-definition time** by `KType::from_type_expr`:
`:(List A B)` rejects with a precise error before the function is ever called.

`KFunction` is not a surface-declarable type name — there's no "any function"
KType, since a function with no signature has nothing to dispatch on. Use
`:(Function (args) -> R)` for typed shapes or `Any` for unconstrained values.
FN's own registered return type is `KType::Any` for the same reason: the
constructed function's projected `ktype()` carries its real shape at runtime.

## Type-position slot kinds

`TypeExprRef` is the meta-type for argument slots that capture a parsed type-name
token (`ExpressionPart::Type(_)`). The slot resolves to a
`KObject::KTypeValue(KType)` carrying the elaborated type — name, nested
parameters, and (for recursive types) `Mu` / `RecursiveRef` structure — so
parameterized types like `:(List Number)` and recursive types like `Tree`
survive the parser → dispatch boundary as a single canonical value. Used by
FN's return-type slot, by STRUCT and UNION's name slots, and by `type_call`'s
verb slot. Slots that want only a bare name (STRUCT/UNION) check the elaborated
shape on the inner value; the validation lives at the consuming builtin rather
than at the slot kind.

## Function signatures

`FN` syntax requires both per-parameter types and a return type:

```
FN (sig) -> ReturnType = (body)
```

Each parameter slot in `<sig>` is written as `name: Type`. A bare identifier
without `: Type` is a parse error — there is no implicit `Any` default. Use
`: Any` to opt a slot out of type-checking. Parameter types are checked at
dispatch via the same `Argument::matches` path as builtins, so a call whose
arguments don't satisfy the signature surfaces as
[`KErrorKind::DispatchFailed`](../../src/machine/core/kerror.rs); the same call shape
with different parameter types routes to a different overload by
slot-specificity (see below).

The return type is non-optional and runtime-enforced. The scheduler injects a
check at user-fn slot finalization that surfaces
[`KErrorKind::TypeMismatch`](../../src/machine/core/kerror.rs) (with a `<return>` arg
name and a frame naming the called function) on mismatch. `Any` is the
no-enforcement fast path for sites that genuinely don't care.

FN itself registers with a return type of `Any` — there's no "any function"
KType to declare, since a function with no signature has nothing to dispatch
on; the constructed function's projected `ktype()` carries the real shape at
runtime.

## Dispatch and slot-specificity

When multiple registered functions match an incoming expression, dispatch picks
by slot-specificity: typed slots outrank untyped ones; literal-typed slots
outrank `Any`. See [expressions-and-parsing.md](../expressions-and-parsing.md) for
how the parser splits an expression into the `Keyword`/slot positions that
specificity scores against.

**Container slots admit on the carried element type, not on shape alone.** An
*unevaluated* container literal (`ListLiteral` / `DictLiteral`) is admitted
shape-only — its element types aren't known until it evaluates. An *evaluated*
container (`Future(List/Dict)`) is admitted only when its memoized carried element
type *satisfies* the slot (`KType::satisfied_by`: exact match or covariant
refinement) — a pure type-level comparison against the value's `ktype()`, with no
element walk. A `List<Number>` value fills `:(List Any)`; a `List<Any>` value (the
join an empty or heterogeneous literal memoizes) fills `:(List Any)` but not
`:(List Number)`. A container whose carried type doesn't satisfy a slot is a
*non-match*: dispatch falls through to outer scopes and, finding nothing, surfaces
`DispatchFailed` rather than committing to a slot that would fail at the bind
boundary.

This makes element-only-differing overloads (`:(List Number)` vs `:(List Str)`)
dispatchable across the forms a container argument takes:

- **Evaluated argument** (`DESCRIBE (xs)`, a call result) — already a typed
  `Future`; the carried-type check picks directly.
- **Bare variable** (`DESCRIBE xs`) — the strict pass *peeks*: a bare `Identifier`
  in a container slot resolves in the dispatch scope, and a name bound to a value is
  admitted on that value's carried element type (`signature_admits_strict` in
  [resolve_dispatch.rs](../../src/machine/core/resolve_dispatch.rs)). The peek
  reuses the `Future` arm of `accepts_part` — `ExpressionPart::Future` holds a
  reference, so no clone. Only container slots peek; binder (`Identifier` /
  `TypeExprRef`) and lazy (`KExpression`) slots never do.
- **Literal** (`DESCRIBE [1 2 3]`) — carries no binding to peek, so it admits both
  overloads shape-only and the strict pass *ties*. The dispatch driver treats a
  strict tie whose argument carries unevaluated eager parts as `Deferred` rather
  than `AmbiguousDispatch`; the literal evaluates and the re-dispatch on the
  resulting typed `Future` is element-aware. A tie that survives evaluation (e.g. an
  empty list against two concrete-element overloads, both admitted vacuously)
  carries no eager parts on the second pass and surfaces as `AmbiguousDispatch`.

The peek reads only an already-bound value. A `Placeholder` (forward reference) or
`Unbound` name isn't peeked, so it falls to the tentative pass — and the two diverge
there:

- A `Placeholder` name *will* bind, so when the tentative pass ties on one, dispatch
  parks on the binder's producer and re-dispatches once it binds, where the now-bound
  type lets the strict-pass peek pick (`ResolveOutcome::ParkOnProducers`, parked
  through the same edges as the resolved-pick replay-park). This keeps dispatch
  order-independent within the visibility window — `DESCRIBE xs` resolves to the
  same overload whether or not `LET xs = …` had landed at first dispatch, provided
  the binding is lexically visible to the reference (see
  [Overload bucket visibility filter](#overload-bucket-visibility-filter)).
- An `Unbound` name names nothing (no visible binding *and* no forward-declared
  placeholder visible at the consumer's chain position), so a tentative tie over it
  can never resolve. It surfaces as the precise `UnboundName` error
  (`ResolveOutcome::UnboundName`), matching what the single-overload path reports
  once its auto-wrapped name evaluates — not a generic dispatch miss.

### Overload bucket visibility filter

Function-bucket lookup pre-filters by per-overload visibility before the strict
admit predicate runs. Each `Bindings::functions` entry carries a per-overload
[`BindingIndex`](../../src/machine/core/bindings.rs) — the lexical statement
index at which the overload was registered, paired with a `nominal_binder` flag.
[`OverloadBucket::pick`](../../src/machine/core/resolve_dispatch.rs) consults
the consumer's [`LexicalFrame`](../../src/machine/core/lexical_frame.rs) chain
and drops any overload whose `BindingIndex` is not visible — same strict
`idx < cutoff` predicate as [`Scope::resolve_with_chain`](../../src/machine/core/scope.rs).
A consumer between two same-bucket overloads sees only the earlier; the
later-sibling overload is hidden, and dispatch falls through to outer scopes
unaffected by the not-yet-visible registration. The `nominal_binder` carve-out
does **not** apply to FN-bucket overloads — they're value-style gated. The
sibling [`pending_overload_producer`](../../src/machine/core/resolve_dispatch.rs)
applies the same per-entry visibility filter when scanning `pending_overloads`
for a not-yet-registered binder to park on.

The result: an FN reference resolves under the same lexical-position rule as a
value-LET reference. Forward calls between sibling FNs work through the
`nominal_binder` carve-out — an FN's name binding is itself nominal so the
call's *resolve* sees it across the sibling cutoff, while the bucket entry for
each new overload remains gated on its own `BindingIndex`. A bare value-LET
forward reference inside a sibling expression surfaces `UnboundName` directly:
visibility is lexical, and the parking edges are reserved for visible-but-not-ready
producers.

## Open work

- **Unified walk + strict-only admission**
  ([roadmap/dispatch_fix/unified-walk.md](../../roadmap/dispatch_fix/unified-walk.md)).
  Collapse the function-candidate ancestor walk and the per-bare-name slot
  walks into one, replace strict-then-tentative with strict-only, and carve
  out the no-keyword shapes into a candidate-machinery-free fast lane.
  Specificity ranking becomes a per-scope tiebreak; innermost-scope wins.

## Known limitations

- **TCO collapses frames.** When A tail-calls B, only B's return type is
  checked at runtime — the slot's `function` field is replaced at TCO time.
- **Builtins are not runtime-checked.** They return through `BodyResult::Value`
  with no slot frame, so the runtime check has nowhere to attach. Their
  declared return types are honest but unenforced.
The two-phase execution work in [open-work.md](open-work.md) closes both
uniformly.
