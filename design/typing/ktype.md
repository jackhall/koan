# `KType` — the runtime type system

[`KType`](../../src/machine/model/types/ktype.rs) has a variant for every concrete `KObject`:

- Scalars: `Number`, `Str`, `Bool`, `Null`.
- Containers: `List(Box<KType>)`, `Dict(Box<KType>, Box<KType>)`,
  `KFunction { params: Record<KType>, ret: Box<KType> }`. Always parameterized; see
  [Container type parameterization](#container-type-parameterization) below.
  `params` is a name-keyed [parameter `Record<KType>`](#record-fields-and-ktype-hashing),
  so a function-typed slot's identity is its parameters by name and type
  (order-blind). The sibling
  `KFunctor { params: Record<KType>, ret, body: Option<&KFunction> }` shares the
  storage and identity rules; the variant tag keeps the two families admissibly
  disjoint (see [functors.md](functors.md)). The `body` is an **identity-inert**
  carrier — `None` for the `:(FUNCTOR …)` type annotation (just a shape), `Some(f)`
  for a bound functor value whose callable rides the type-table identity so a later
  `:(F {…})` / `F {…}` application can invoke it. `body` is excluded from equality,
  hashing, admissibility, join, and rendering, all of which compare `params` + `ret`
  only, so two structurally-identical functor types compare and hash equal
  regardless of body. When a function's source return is
  per-call-elaborated, its `ret` box holds a `DeferredReturn(DeferredReturnSurface)`
  carrier — see [Record fields and `KType` hashing](#record-fields-and-ktype-hashing).
- Structural record: `Record(Box<Record<KType>>)` — an identifier-keyed field schema
  (`:{x :Number, y :Str}`), distinct from a nominal `Struct`-kind `SetRef`
  (records are structural, structs nominal). A record *value* (`KObject::Record`,
  surface `{x = 1, y = "a"}`) memoizes its per-field type record as its carried type.
  Width/depth subtyping orders record *values* in the dispatch lattice — see
  [Variance](#variance).
- Other function-like: `KExpression` (a captured-but-unevaluated expression).
- Meta-type for type-position slots: `OfKind(KKind)` — a type-accepting slot carries
  a shallow [`KKind`](../../src/machine/model/types/kkind.rs) expectation, and a type value
  flowing in the value channel's `Type` arm is classified by `KType::kind_of` and matched
  against it by subsumption. `OfKind` is **type-channel-only**: it admits a type value, never
  a runtime instance — a value is matched by a type, never by a kind. The kinds form one
  subsumption lattice, `Any > {Module, Signature, Proper > {Tagged, Newtype, TypeConstructor}}`:
  a parsed type-name slot is `OfKind(Proper)`, the `:Type` surface is `OfKind(Any)`, the
  `:Module` / `:Signature` wildcards are `OfKind(Module)` / `OfKind(Signature)`, and the three
  nominal families sit strictly below `Proper`. `KKind::admits` is reflexive subsumption (a
  `Proper` / `Any` slot admits any proper-subtree type value, while the module/signature wall
  keeps each of those families to itself); `KKind::strictly_below` orders specificity, so an
  `OfKind(Tagged)` slot out-specifies an `OfKind(Proper)` sibling. See
  [Type-position slot kinds](#type-position-slot-kinds).
- First-class type values: a type flows raw as a `&KType` in the value channel's `Type`
  arm — there is no `KObject` box. As a parameter-slot annotation, `OfKind(Proper)` (`:Type`'s
  `OfKind(Any)` likewise) admits any *proper* type value: bare builtin type tokens (`Number`,
  `Str`, `Bool`, `Null`), tagged-union and struct nominal tokens, and any other non-module /
  non-signature type. Modules and signatures route through the dedicated `OfKind(Module)` /
  `OfKind(Signature)` / `Signature { .. }` slots so the `:Type` vs `:Module` overload
  distinction stays intact — see
  [`KType::accepts_part`](../../src/machine/model/types/ktype_predicates.rs)
  and the pin test
  [`type_slot_admits_bare_builtin_tokens_and_user_type_carriers`](../../src/machine/model/types/ktype_predicates/tests.rs).
- User-declared nominal types — three variants reference members of an
  `Rc`-owned [`RecursiveSet`](../../src/machine/model/types/recursive_set.rs),
  the atomic unit of nominal allocation, identity, and lift (one strongly-connected
  component of mutually-recursive types; a non-recursive type is a singleton set).
  See [user-types.md](user-types.md) for the full model.
  - `SetRef { set: Rc<RecursiveSet>, index }` — the **external** handle, the
    per-declaration identity synthesized by `KObject::ktype()` for `Struct` and
    `Tagged` carriers and held by `bindings.types`. Identity is
    `(Rc::as_ptr(set), index)` — never the schema, which may be cyclic. Two
    distinct STRUCTs sit at distinct `(set ptr, index)` pairs, giving the
    per-declaration-distinctness dispatch keys on. The member's `kind` (read via
    `set.member(index).kind`) is one of the nominal families `KKind::{Tagged, Newtype,
    TypeConstructor}` — `kind_of` reads it off the `SetRef` to classify the nominal type
    value. Lift `Rc::clone`s the whole set, so the recursive group
    travels as one cycle-aware unit.
  - `SetLocal(index)` — the **intra-set sibling** reference inside a member's
    schema, a bare index resolved against the ambient set during deep traversal.
    It carries no `Rc` (so the set holds no internal refcount cycle) and never
    reaches the predicates — matching is shallow `SetRef` identity that does not
    descend a member's schema.
  - `RecursiveGroup(Rc<RecursiveSet>)` — the first-class handle to a whole set,
    bound by a `RECURSIVE TYPES` group name. Identity is the set pointer
    (`Rc::ptr_eq`); inert in value dispatch.
  - `Variant { set: Rc<RecursiveSet>, index, tag }` — a **refinement** of a
    `Tagged`-kind member: `(set, index)` names the union, `tag` selects one
    variant. Identity is `(Rc::as_ptr(set), index, tag)` — the union member plus
    tag, never the schema. It is what a user-`UNION` value's `ktype()` reports and
    what a `:(Maybe Some)` slot carries; a variant is strictly more specific than
    its union's `SetRef`. Lift `Rc::clone`s the whole set like `SetRef`. Variant
    tags are capitalized `Type` tokens, so a variant is type-classified
    everywhere. See [user-types.md § Tagged-union variants](user-types.md#tagged-union-variants).

  A slot that wants "any user-declared type of family X" is an `OfKind(KKind)`
  carrying the nominal family (`OfKind(Tagged)` / `OfKind(Newtype)` /
  `OfKind(TypeConstructor)`). Because `OfKind` is type-channel-only, such a slot
  admits the *type value* of that family, not a runtime instance — a builtin that
  dispatches on a runtime representation (ATTR's newtype field access) takes the
  least-specific `Any` slot and validates the `KObject::Wrapped` shape in its body
  (`access_field`), never matching the value by a kind. The nominal-family surface
  keywords (`Tagged` / `Newtype` / `TypeConstructor`) are pinned for diagnostic
  rendering only — none is registered as a writable surface name (no entry in
  [`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs)).
- `RecursiveRef(String)` — a **definition-time transient only**: a self or
  forward-sibling reference lowers to it during elaboration and the member's
  finalize seals it to `SetLocal(index)`. It never appears in a sealed type and
  never reaches the predicates. Equality is by name only.
- Module / signature carriers (the [module system](modules.md) rests on
  these): `Module { module: &'a Module<'a>, frame: Option<Rc<CallArena>> }`
  is the first-class module value's type — the arena-pinned `&Module`
  pointer plus the per-call frame anchor for functor-built modules;
  `Signature { sig: &'a Signature<'a>, pinned_slots: Vec<(String, KType)> }`
  serves both signature roles in one variant — the introspectable value
  (carrying `decl_scope` via `sig`) *and* the dispatch constraint ("any
  module satisfying this signature"); `AbstractType { source:
  AbstractSource<'a>, name: String }` is the per-abstract-type-member tag.
  `AbstractSource` is `Sig(ScopeId) | Module(&'a Module<'a>)`: a
  `Sig`-rooted member is named at SIG-declaration time (a SIG-local
  `LET Type = ...` that would otherwise collapse to its underlying type binds
  this name-bearing tag instead), while a `Module`-rooted member is the per-call
  mint `:|` opaque ascription produces (`Foo.Type`, with a module to project
  further members off). Manual `PartialEq` keys identity on
  `module.scope_id()` for `KType::Module`, `sig.sig_id()` + `pinned_slots`
  for `KType::Signature` (`sig.path` is diagnostic-only), and
  `(source.scope_id(), name)` for `KType::AbstractType` — so two
  opaque ascriptions of the same source module produce distinct
  `KType::Module` identities (the abstraction barrier) but their
  `AbstractType` minting for the same slot name compares equal, and a
  per-call `Module`-rooted mint stays distinct from the `Sig`-rooted member it
  was threaded from.
  Companion wildcards `OfKind(Module)` and `OfKind(Signature)` admit any module
  or signature value respectively; the surface keywords `Module` and
  `Signature` lower to them in
  [`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs).
  The single `Signature` variant is **disambiguated by position**: a
  `Signature { .. }` *slot* matches a *module* whose `compatible_sigs`
  contains `sig.sig_id()` (the constraint role — what `Er :OrderedSig`
  lowers to in a FUNCTOR parameter slot, so `:OrderedSig` means "module
  satisfying OrderedSig," never "the signature value itself"), while a
  signature *value* (a `KType::Signature { .. }` flowing in the `Type` arm) is matched only
  by the `OfKind(Signature)` wildcard. `pinned_slots` (empty for a bare
  signature) carries `WITH` abstract-type specializations; because the
  same variant rides a live `&Signature`, a `WITH` result is
  introspectable too.
- Higher-kinded application: `ConstructorApply { ctor: Box<KType>, args:
  Vec<KType> }` — structural identity by `(ctor, args)`, mirror of `List(_)`
  / `Dict(_, _)`. `ctor` is a `SetRef` to a `TypeConstructor`-kind member.
  Emitted by `elaborate_type_expr` when the outer name of a parameterized type
  expression resolves to such a member; renders as `ctor<arg1, arg2>` in
  diagnostics. See
  [functors.md § Higher-kinded type slots](functors.md#higher-kinded-type-slots)
  for the surface form and per-call generativity.
- `Any` — the no-op fast-path.

[`KType::matches_value`](../../src/machine/model/types/ktype_predicates.rs) plus
[`KObject::ktype`](../../src/machine/model/values/kobject.rs) close the loop on runtime
checking: every value has a queryable type, and any declared type can be checked
against it.

## Container type parameterization

`:(LIST OF T)`, `:(MAP K -> V)`, and `:(FN (args) -> ret)` carry their inner
types on the variant directly. `KType` is not `Copy`; structural payloads are
`Box`ed where the variant would otherwise be self-referential.

**Surface syntax** is a glued-right `:` sigil opening an S-expression
type-expression group. The parser treats `:(...)` as a parse-context marker
anchored to the `:` — a `:(...)` sigil emits one
[`ExpressionPart::SigiledTypeExpr(Box<KExpression>)`](../../src/machine/model/ast.rs)
wrapping the raw inner expression verbatim, with no shape recognition at
parse time. (The one structurally-recognized sigil is `:{…}`, which emits a
first-class `ExpressionPart::RecordType` instead — see
[type-language-via-dispatch.md § Record-type sigil](type-language-via-dispatch.md#record-type-sigil).)
Shape decisions (keyworded `:(LIST OF Number)`, user-functor
`:(MyFunctor {T = IntOrd})`, etc.) are the dispatcher's responsibility — the
parser's only job is to flag "this slot evaluates to a type". `<` and `>` flow through unencumbered as keyword
tokens, leaving the arithmetic comparison operators available. The framing
logic lives in [frame.rs](../../src/parse/frame.rs) (`Frame::TypeExpr`);
the dispatcher's `fast_lane_sigiled_type_expr` handler
([dispatch.rs](../../src/machine/execute/dispatch.rs))
tail-replaces the slot with a `Dispatch` of the wrapped expression. See
[type-language-via-dispatch.md](type-language-via-dispatch.md) for the full
sigil-and-dispatch contract.

**Keyworded surface overloads** for the four builtin parameterized
constructors — `LIST OF`, `MAP _ -> _`, `FN <sig> -> _`, and
`FUNCTOR <sig> -> _` — register in
[`builtins/type_constructors.rs`](../../src/builtins/type_constructors.rs)
and produce `KType::...` results in the value channel's `Type` arm; they are the canonical
type-language surface, dispatched and assembled as ordinary sub-expressions
through the type-language path. (A module type-member is named by the dotted
`M.T` access and signature specialization by the infix `WITH {…}` — neither is
an underscore builtin.)

### Variance

Variance is split across the parameterized constructors. `List` and `Dict` are
covariant in their parameter positions. `Function` is **contravariant in its
parameter record (with width drop) and covariant in its return** — sound
function subtyping reasoned against call-by-name invocation, where a parameter
arrives name-keyed and a value fills a slot by being usable wherever the slot's
type is expected. The split falls out of the underlying check in each case
rather than being a deliberate design dial — each choice is the natural one
given how the constructor's values are matched.

Three sites consume parameterized types, and each has its own behavior:

| Site | What it does | Variance |
| --- | --- | --- |
| `matches_value` | Walks a runtime value against a declared type at an ascription boundary (FN return, FN argument, `LET`). | **Covariant** for `List` / `Dict`: `:(LIST OF Any)` accepts any list because `Any.matches_value(_)` is always true; `:(MAP Str -> Any)` accepts a `{a: 1, b: "x"}` value. **Invariant** for `Function`: delegates to `function_compat`. |
| `is_more_specific_than` | Ranks two slot types when multiple overloads match the same call. Used by `specificity_vs` to break dispatch ties. Concrete carrier types also outrank the unconstrained-name slot types `Identifier` and `OfKind(Proper)`, so an `ATTR <s:Struct>` overload beats an `ATTR <s:Identifier>` fallback when both admit. | **Covariant** for `List` / `Dict` (element, key, value): `:(LIST OF Number)` ≺ `:(LIST OF Any)`, `:(MAP Str -> Number)` ≺ `:(MAP Str -> Any)`. **Contravariant params (with width-subset) + covariant return** for `Function` / `Functor`, matching `function_compat`: `:(FN (x :Any) -> Str)` ≺ `:(FN (x :Number) -> Str)` (more-general param wins), `:(FN (x) -> Number)` ≺ `:(FN (x) -> Any)` (narrower return wins), and a nullary `:(FN () -> R)` ≺ a unary `:(FN (x) -> R)` (narrower width wins). |
| `function_compat` | The dispatch-time check that a `KObject::KFunction` value fills a typed function-shaped slot. | **Function subtyping** — contravariant params (width + depth) + covariant return. A value `(x :Any) -> Str` fills a slot typed `:(FN (x :Number) -> Str)`; a value `(x :Number) -> Number` fills `:(FN (x :Number) -> Any)`; a unary value fills a binary slot (the extra slot param arrives unbound under call-by-name). A value requiring a param the slot doesn't promise is a non-match. |

Admission (`function_compat`) and specificity (`is_more_specific_than`) share
**one** relation for function slots — contravariant params with width-subset,
covariant return — so most-specific-wins is consistent: the same value can now
fill several function slots at once (e.g. an `(x :Any) -> R` value fills both
`:(FN (x :Number) -> R)` and `:(FN (x :Any) -> R)`), and the ranking orders
those slots the same way admission does. Where one admitting slot is strictly
more specific than the others it wins outright; where two admitting slots are
genuinely incomparable — an `(x :Any) -> R` value against both
`:(FN (x :Number) -> R)` and `:(FN (x :Str) -> R)`, neither more specific —
dispatch ties and surfaces `AmbiguousDispatch`. The `List` / `Dict` covariance
is observable the same way: `(xs :(LIST OF Number))` strictly outranks
`(xs :(LIST OF Any))` for a number-list call.

**Return admission splits on whether the value's return is resolved or
deferred.** A `Resolved` value return admits covariantly as above — `sig_ret ==
ret || sig_ret ≺ ret`. A *deferred* value return (a per-call-elaborated functor
return like `-> Er`) carries no resolved `KType`, so `function_compat` admits it
by **syntactic equality of its surface shadow**: an `Any` slot admits any
deferred return; a slot whose `ret` is a `KType::DeferredReturn` carrier admits
iff its `DeferredReturnSurface` shadow equals the candidate's; any resolved slot
rejects, since a deferred return is opaque until per-call elaboration and refines
nothing more precise than its own shadow. The specificity short-circuit
`DeferredReturn ≺ Any` (covariant, via the `Any` arm) keeps a deferred-return
slot strictly more specific than an `Any`-return one.

**Record values subtype the dual way to function params.** A record value is
ranked by `record_value_more_specific`
([ktype_predicates.rs](../../src/machine/model/types/ktype_predicates.rs)): a
*wider* record is **more specific** — a `{x = 1, y = "a"}` value (carried type
`:{x :Number, y :Str}`) fills a narrower `:{x :Number}` slot by dropping `y`, so the
superset arm wins a dispatch tie. Depth is **covariant** in the field types
(`:{x :Number}` ≺ `:{x :Any}`), sound because koan values are immutable
([memory-model](../memory-model.md)). The relation is the dual of
`param_record_more_specific` (contravariant params with width-*drop* for
call-by-name) — records and function params share the `Record` substrate but order
opposite ways, so the two helpers stay separate. Incomparable record arms
(`:{x :Number, y :Str}` vs `:{x :Number, z :Str}`, filled by a value carrying all of
`x`, `y`, `z`) tie as `AmbiguousDispatch`; the [`FROM` projection
builtin](../../src/builtins/record_projection.rs) breaks the tie at the call site —
`(x y) FROM r` re-tags the record value's carried field-type record to exactly the
named fields (`Rc`-sharing the backing value record whole), so only the `:{x, y}` arm
admits. Admission mirrors `List` / `Dict`: an unevaluated `{x = …}` literal admits
shape-only, while an evaluated record compares its memoized field-type record against
the slot via `satisfied_by` (no field walk).

Concretely:

```
LET nums = [1 2 3]

FN (PICK xs :(LIST OF Any))    -> Str = ("any")
FN (PICK xs :(LIST OF Number)) -> Str = ("number")

PICK nums   # → "number"   (covariant: :(LIST OF Number) ≺ :(LIST OF Any))
```

```
FN (BAD) -> :(LIST OF Number) = ([1 "x"])
BAD   # → TypeMismatch: expected :(LIST OF Number), got :(LIST OF Any)
        # (matches_value walks elements; covariant — Any.matches_value(_) is true,
        #  Number.matches_value("x") is false)
```

```
FN (USE f :(FN (x :Number) -> Str)) -> Str = ("got fn")

USE (FN (SHOW x :Number) -> Str = ("hi"))   # → "got fn"   (function_compat: equal by name+type)
USE (FN (SHOW x :Any)    -> Str = ("hi"))   # → "got fn"   (contravariant param: a value
                                            #   accepting Any fills a slot promising only Number)
```

```
FN (USE f :(FN (x :Number, y :Str) -> Str)) -> Str = ("got fn")

USE (FN (SHOW x :Number) -> Str = ("hi"))   # → "got fn"   (width drop: a unary value fills a
                                            #   binary slot; the extra slot param `y` arrives
                                            #   unbound under call-by-name)
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
their declared signature (`KObject::KFunction(f, _)` → `KFunction { params, ret }`,
the parameter record read off `f.signature`'s named slots). `KType::join` joins
two same-shape `KFunction`s (and same-shape `KFunctor`s) name-keyed, coarsening a
mismatched parameter-name set or a function-vs-functor pair to `Any`.

**Empty containers carry no element type to infer**, so an unstamped empty `[]`
/ `{}` (element type memoized as `Any`, never stamped by an annotation) is an
**error** at an untyped resolution boundary — an untyped value-route `LET`, a
bare top-level expression result. The producing boundary must annotate the value
(e.g. a typed FN return) or use a non-empty literal. A *stamped* empty container
(an `FN -> :(LIST OF Number) = ([])` whose carrier is re-tagged to element `Number`)
is fine; a heterogeneous non-empty literal (`[2, "hello"]` → `List<Any>`) is
unaffected — it carries information and is legal where `:(LIST OF Any)` is declared.

### Runtime type-parameter carriers

`List`, `Dict`, and `Tagged` carry their runtime type arguments on the variant so
dispatch and slot admission see the full instantiation, not just the outer shape:

- `KObject::List(items, elem)` / `KObject::Dict(map, key, value)` memoize the
  element / key+value type at construction (`KObject::list` / `KObject::dict`).
- `KObject::Tagged { type_args, .. }` carries the applied type arguments of a
  parameterized union (`Result<T, E>`). Empty `type_args` means erased — `ktype()`
  reports the bare `SetRef`; a populated carrier makes `ktype()`
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
value crossing an annotated boundary is checked via `matches_value`. Where the
boundary also re-tags, it stamps (`KObject::stamp_type`) the carrier to *exactly*
the declared type, **coarsening included** — a `List<Number>` value returned
through `:(LIST OF Any)` re-tags to `List<Any>`, so downstream dispatch sees the
contract rather than the
implementation's incidental precision. An unannotated value keeps its precise
memoized type; surrendering precision is the deliberate act of writing an
annotation. The three boundaries are:

- **FN return** — the returned value is walked with `matches_value` against the
  declared return type (a list literal `[1, "x"]` returned where `:(LIST OF Number)`
  was declared fails with a structured `TypeMismatch` naming both types). For a
  **resolved** return type the lift-time Done boundary in
  [`scheduler/execute.rs`](../../src/machine/execute/scheduler/execute.rs) then
  stamps the carrier to the declared type (`check_declared_return` →
  `KObject::stamp_type`). The **deferred**-return path checks only:
  [`check_deferred_return`](../../src/machine/core/kfunction/exec.rs) runs
  `matches_value` and passes a satisfying value through un-stamped (a passing value
  already satisfies the declared type, at worst as a subtype).
- **FN argument** — each parameterized-carrier argument slot (`List` / `Dict` /
  `ConstructorApply`) is checked with `matches_value` in
  [`KFunction::validate_call_args`](../../src/machine/core/kfunction.rs) before the
  body binds — a uniquely-picked call is admitted shape-only by dispatch, so this is
  where a non-satisfying typed argument becomes a hard `TypeMismatch` rather than
  slipping through. The check is not followed by an argument stamp. This
  `matches_value` walk is the authoritative content-recursive check; for `List` /
  `Dict` it confirms what dispatch already gates, since an evaluated container whose
  carried element type doesn't satisfy the slot is rejected as a dispatch non-match
  (see [Dispatch and slot-specificity](#dispatch-and-slot-specificity)).
- **`LET`** ascription — same check-then-stamp on the bound value.

**Parameter arity is fixed by the keyworded sigil shape.** `:(LIST OF X)`
carries exactly one element slot and `:(MAP K -> V)` exactly two, so an
arity mismatch isn't expressible at the surface — the type-constructor
overloads only match the well-formed shape, and any other arrangement
fails to resolve as a parameterized type at all. (See
[elaboration.md § Layers](elaboration.md#layers) § Layer 1 for where type
elaboration sits in the pipeline.)

`KFunction` is not a surface-declarable type name — there's no "any function"
KType, since a function with no signature has nothing to dispatch on. Use
`:(FN (args) -> R)` for typed shapes or `Any` for unconstrained values.
FN's own registered return type is `KType::Any` for the same reason: the
constructed function's projected `ktype()` carries its real shape at runtime.

## Type-position slot kinds

`OfKind(Proper)` is the meta-type for argument slots that capture a parsed type-name
token (`ExpressionPart::Type(_)`). The slot resolves to a `&KType` flowing raw in the value
channel's `Type` arm, carrying the elaborated type — name, nested
parameters, and (for recursive types) the `SetRef` into a sealed `RecursiveSet` —
so parameterized types like `:(LIST OF Number)` and recursive types like `Tree`
survive the parser → dispatch boundary as a single canonical value. Used by
FN's return-type slot, by STRUCT and UNION's name slots, and by `type_call`'s
verb slot. Slots that want only a bare name (STRUCT/UNION) check the elaborated
shape on the inner type; the validation lives at the consuming builtin rather
than at the slot kind.

### `KType::Unresolved` — surface form survives bind

A type-position value whose surface `TypeName` doesn't resolve at
`ExpressionPart::resolve_for` time — a bare-leaf name outside
[`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs)'s
builtin table (`Point`, `IntOrd`, `MyList`, or an unknown name like
`SomeWeirdName`) — rides through bind as the
[`KType::Unresolved(TypeName)`](../../src/machine/model/types/ktype.rs)
transient in the `Type` arm rather than a resolved `&KType`. See
[elaboration.md § Layers](elaboration.md#layers) § Layer 5 for where this
transient sits in the pipeline and the eventual scope-aware elaboration
hop.

The guarantee this gives consumers: diagnostics can quote the user's
identifier exactly as written, not the elaborated canonical form. A FN
declared `FN (DOIT) -> SomeWeirdName = (1)` whose return-type name never
binds surfaces a `ShapeError` mentioning `SomeWeirdName` verbatim, not a
synthesized rewrite. The same applies to user-bound aliases like `MyT` —
the carrier remembers `MyT` as written, and only at the resolution boundary
does it elaborate to the underlying type. Pinned by
`fn_return_type_surface_name_preserved_in_error` in
[`src/builtins/fn_def/tests/return_type.rs`](../../src/builtins/fn_def/tests/return_type.rs).

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
no-enforcement fast path for sites that genuinely don't care. `MATCH` and `TRY`
arms share this check: their mandatory `-> :T` rides the same slot carrier (a
[`ReturnContract`](../../src/machine/core/kfunction/body.rs) — `Function` for a
call, `Arm` for a function-less arm) and the same Done-arm check, so every arm
agrees on `T` and the expression's value carries `T` for downstream dispatch (see
[execution-model.md § Arms as own blocks](../execution-model.md#arms-as-own-blocks)).

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
element walk. A `List<Number>` value fills `:(LIST OF Any)`; a `List<Any>` value (the
join an empty or heterogeneous literal memoizes) fills `:(LIST OF Any)` but not
`:(LIST OF Number)`. A container whose carried type doesn't satisfy a slot is a
*non-match*: dispatch falls through to outer scopes and, finding nothing, surfaces
`DispatchFailed` rather than committing to a slot that would fail at the bind
boundary.

This makes element-only-differing overloads (`:(LIST OF Number)` vs `:(LIST OF Str)`)
dispatchable across the forms a container argument takes. Admission is
strict-only, driven by a per-`run_dispatch` `bare_outcomes` cache —
[`signature_admits_strict`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
reads each bare-name slot's cached
[`NameOutcome`](../../src/machine/execute/dispatch/resolve_dispatch.rs) once and
admits accordingly. The forms:

- **Evaluated argument** (`DESCRIBE (xs)`, a call result) — already a typed
  `Future`; admission runs `arg.matches(part)` and `accepts_part` for the
  carried-type check.
- **Bare variable** (`DESCRIBE xs`) — the cache entry is
  `NameOutcome::Resolved(Carried)`. Admission tests
  [`KType::accepts_part`](../../src/machine/model/types/ktype_predicates.rs)
  against `ExpressionPart::Future(Carried)` (the `Future` arm holds a `Carried`
  reference — an object or a `Type` arm — no clone). A bare name whose value has the
  wrong carrier type strict-rejects the overload; the call surfaces as `DispatchFailed`
  rather than a bind-time `TypeMismatch`. Binder (`Identifier` / `OfKind(Proper)`) and
  lazy (`KExpression`) slots skip the cache and admit shape-only — the slot
  owns the name, so admission can't depend on whether `x` happens to be
  bound or parked.
- **Literal** (`DESCRIBE [1 2 3]`) — the cache entry is `None` (literals
  aren't bare names) and admission runs `arg.matches(part)` shape-only.
  Both element-typed overloads admit and the strict pass *ties*. The
  dispatch driver treats a strict tie whose argument carries unevaluated
  eager parts as `Deferred` rather than `AmbiguousDispatch`; the literal
  evaluates and the re-dispatch on the resulting typed `Future` is
  element-aware. A tie that survives evaluation (e.g. an empty list
  against two concrete-element overloads, both admitted vacuously)
  carries no eager parts on the second pass and surfaces as
  `AmbiguousDispatch`.

`Placeholder` (forward reference) and `Unbound` cache outcomes admit via
shape-only `arg.matches(part)` rather than carrier-type check. The
post-pick splice/park walk is the only place that produces precise per-slot
`ParkOnProducers` / `UnboundName` diagnostics, so admission must not
reject them. If no bucket admits anywhere, the resolver's post-walk
fallback reads the cache by fixed precedence — placeholders > eager >
unbound > pending overload > Unmatched — and surfaces the right
`ResolveOutcome`:

- A `Placeholder` name *will* bind, so the fallback surfaces
  `ResolveOutcome::ParkOnProducers(producers)`. Dispatch parks on the
  binder's producer and re-dispatches once it binds; the rebuilt cache
  carries `Resolved(obj)` and strict admission picks. This keeps dispatch
  order-independent within the visibility window — `DESCRIBE xs` resolves
  to the same overload whether or not `LET xs = …` had landed at first
  dispatch, provided the binding is lexically visible to the reference
  (see [Overload bucket visibility filter](#overload-bucket-visibility-filter)).
  Park parking goes through the same edges as the resolved-pick
  replay-park.
- An `Unbound` name names nothing (no visible binding *and* no
  forward-declared placeholder visible at the consumer's chain position),
  so the fallback surfaces `ResolveOutcome::UnboundName(name)` — the
  precise error matching what the single-overload path reports for an
  unresolved bare name, not a generic dispatch miss.

Specificity ranks `is_more_specific_than` so that concrete carrier types
beat the unconstrained-name slot types (`Identifier` / `OfKind(Proper)`). A
call like `ATTR p z` where `p` resolves to a `Struct` admits both a
concrete `ATTR <s:Struct>` overload and an `ATTR <s:Identifier>` fallback;
the concrete overload wins by specificity without tying.

### Overload bucket visibility filter

Function-bucket lookup pre-filters by per-overload visibility before the strict
admit predicate runs — the [lookup → admit protocol](lookup-protocol.md)'s
Layer 2 (`Bindings::lookup_function`) applied per-overload rather than per
name. Each `functions` entry carries a per-overload
[`BindingIndex { idx }`](../../src/machine/core/bindings.rs) — the lexical
statement index at which the overload was registered. The visibility predicate
is `idx < cutoff`, one rule across the value and type languages. A consumer
between two same-bucket overloads sees only the earlier; the later-sibling
overload is hidden, and dispatch falls through to outer scopes unaffected by the
not-yet-visible registration.
[`OverloadBucket::pick_strict`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
receives the pre-filtered survivor list (the `FunctionLookup`'s `overloads`)
and runs only the admit predicate over it. The same lookup also surfaces the
earliest-index visible `pending_overloads[key]` producer in `FunctionLookup`'s
`pending` field; a visible pending parks that scope for a park-and-replay on
wake, since it would shadow once finalized.

The result: an FN reference resolves under the same lexical-position rule as a
value-LET reference, and a bare forward reference inside a sibling expression
surfaces `UnboundName` directly — visibility is lexical, and the parking edges
are reserved for visible-but-not-ready producers.

## Record fields and `KType` hashing

A struct schema's fields are a [`Record<V>`](../../src/machine/model/types/record.rs) —
an ordered identifier-keyed map, generic over its value, so the type level stores
`Record<KType>` and a value level can later store `Record<KObject>`.
A set member's [`NominalSchema::Struct(Record<KType>)`](../../src/machine/model/types/recursive_set.rs)
carries the field record by value; the `STRUCT` elaborator wraps the parser's
declaration-ordered `(name, KType)` pairs into a `Record` once, at the
`finalize_struct` boundary, and fills the member's schema cell.

The same `Record<KType>` substrate backs `KFunction` / `KFunctor` parameter
identity: both variants store their parameters as `params: Record<KType>`
(`(name → type)`), built by `finalize_carrier` in
[`type_constructors.rs`](../../src/builtins/type_constructors.rs) from the
shared field-list parser STRUCT / UNION use. A function-typed slot is thus
identified by its parameter names and types order-blind — `:(FN (x :Number,
y :Str) -> Bool)` equals `:(FN (y :Str, x :Number) -> Bool)`. Function
admission compares the two records under width-drop subtyping (see
[Variance](#variance)): a value that requires a parameter the slot doesn't
declare is a non-match, while extra *slot* parameters the value doesn't declare
are fine — they arrive unbound under call-by-name. `KType::join` reuses the
record join for both arms.

The shape has two defining properties:

- **Insertion order is preserved** for rendering and positional construction
  (`Record::iter` walks declaration order), but **equality ignores it**:
  `(x :Number, y :Str)` and `(y :Str, x :Number)` are the same record. The
  order-blind `PartialEq` is `IndexMap`'s, forwarded directly. Names are unique
  within a record — a structural property `IndexMap` keys carry for free, and one
  `parse_pair_list` already enforces by rejecting a duplicate field name.
- **Hashing agrees with that order-blind equality**: a commutative fold
  (`wrapping_add`) over a per-field `mix(hash(name), hash(value))`. The `mix` binds
  name to value before the fold, so `{x: Number}` and `{y: Number}` hash apart; the
  symmetric accumulator makes the result independent of field order. Wrapping-add
  rather than XOR, which would cancel on a duplicate.

`Record<V>: Hash` needs `V: Hash`, so `KType` implements `Hash`, kept consistent with
its hand-written `PartialEq` arm-for-arm: the discriminant leads (so distinct variants
never alias and the unit variants need no further mixing), then each compound arm
hashes exactly the fields its `PartialEq` arm compares. The pointer-identity
variants hash their stable identity key — `Module` hashes `scope_id()`,
`AbstractType` hashes its `source.scope_id()`, `Signature` hashes `sig_id()`,
`SetRef` hashes `(Rc::as_ptr(set), index)` and `RecursiveGroup` hashes
`Rc::as_ptr(set)` — never descending the (possibly cyclic) member schema, so
hashing terminates and agrees arm-for-arm with the pointer-keyed `PartialEq`.

`KType::DeferredReturn(DeferredReturnSurface)` is a confined hashable leaf: it
holds the type-language shadow of a per-call-elaborated function return —
`TypeExpr(TypeName)` for parser-preserved leaf forms, `Expression(String)` for
the canonical `summarize()` render of a parens-form return (the live
`KExpression` impls neither `Eq` nor `Hash`). It hashes and compares by that
shadow, so two functions differing only in their deferred returns are distinct
structural types. The variant is valid *only* inside a synthesized
`KFunction` / `KFunctor` `ret` box that `function_value_ktype` builds; no runtime
value's `ktype()` returns it free-standing, and it admits nothing on its own
(`accepts_part` is `false`).

The same `Record<V>` substrate also backs the first-class structural record type
`KType::Record(Record<KType>)` and its value `KObject::Record(Record<KObject>, …)`
(surface `{x = 1, y = "a"}`). The dict carrier (`KType::Dict`, `KObject::Dict`) stays
a sibling: records restrict keys to identifiers and admit heterogeneous per-field
types, while dicts admit arbitrary value keys and one homogeneous value type. The two
never share a key representation, and the value surfaces disambiguate at parse time —
a brace literal with `=` pairs (`{x = 1}`) is a record, with `:` pairs (`{k: v}`) a
dict (see [type-language-via-dispatch.md § Record-type sigil](type-language-via-dispatch.md#record-type-sigil)).

## Known limitations

- **TCO collapses frames.** When A tail-calls B, only B's return type is
  checked at runtime — the slot's `ReturnContract` carrier is replaced at TCO
  time. A nested `MATCH` / `TRY` arm whose body tail-calls a function is checked
  against the callee's contract, not the arm's `-> :T`.
- **Value-returning builtins are not runtime-checked.** They return through
  a `Done` value with no slot frame, so the runtime check has nowhere to
  attach; their declared return types are honest but unenforced. `MATCH` / `TRY`
  are the exception — they return through an `Action::Tail` carrying a
  `ReturnContract::Arm`, so their `-> :T` is enforced.
The two-phase execution work in [open-work.md](open-work.md) closes both
uniformly.

## Open work

None tracked.
