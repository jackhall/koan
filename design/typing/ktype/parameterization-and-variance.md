# Parameterization, variance, and runtime carriers

Container type parameterization, the variance lattice that orders slot
specificity, and the runtime carriers for type parameters. Part of the
[`KType` reference](README.md).

## Container type parameterization

`:(LIST OF T)`, `:(MAP K -> V)`, and `:(FN :{args…} -> ret)` carry their inner
types on the variant directly. `KType` is not `Copy`; structural payloads are
`Box`ed where the variant would otherwise be self-referential.

**Surface syntax** is a glued-right `:` sigil opening an S-expression
type-expression group. The parser treats `:(...)` as a parse-context marker
anchored to the `:` — a `:(...)` sigil emits one
[`ExpressionPart::SigiledTypeExpr(&KExpression)`](../../../src/machine/model/ast.rs)
wrapping the raw inner expression verbatim, with no shape recognition at
parse time. (The one structurally-recognized sigil is `:{…}`, which emits a
first-class `ExpressionPart::RecordType` instead — see
[type-language-via-dispatch.md § Record-type sigil](../type-language-via-dispatch.md#record-type-sigil).)
Shape decisions (keyworded `:(LIST OF Number)`, nominal construction
`:(MyStruct {x = 1})`, etc.) are the dispatcher's responsibility — the
parser's only job is to flag "this slot evaluates to a type". `<` and `>` flow through unencumbered as keyword
tokens, leaving the arithmetic comparison operators available. The framing
logic lives in [frame.rs](../../../src/parse/frame.rs) (`Frame::TypeExpr`);
the dispatcher's `sigiled_type_expr` handler
([decide.rs](../../../src/machine/execute/decide.rs))
tail-replaces the slot with a `Dispatch` of the wrapped expression. See
[type-language-via-dispatch.md](../type-language-via-dispatch.md) for the full
sigil-and-dispatch contract.

**Keyworded surface overloads** for the three builtin parameterized
constructors — `LIST OF`, `MAP _ -> _`, and `FN <sig> -> _` — register in
[`builtins/parameterized_types.rs`](../../../src/builtins/parameterized_types.rs)
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
| `is_more_specific_than` | Ranks two slot types when multiple overloads match the same call. Used by `specificity_vs` to break dispatch ties. Concrete carrier types also outrank the unconstrained-name slot types `Identifier`, `NameToken`, `TypeNameToken` and `OfKind(Proper)`, so a concrete-typed `ATTR` overload beats an `ATTR <s:Identifier>` fallback when both admit. | **Covariant** for `List` / `Dict` (element, key, value): `:(LIST OF Number)` ≺ `:(LIST OF Any)`, `:(MAP Str -> Number)` ≺ `:(MAP Str -> Any)`. **Contravariant params (with width-subset) + covariant return** for `Function`, matching `function_compat`: `:(FN :{x :Any} -> Str)` ≺ `:(FN :{x :Number} -> Str)` (more-general param wins), `:(FN :{x} -> Number)` ≺ `:(FN :{x} -> Any)` (narrower return wins), and a nullary `:(FN :{} -> R)` ≺ a unary `:(FN :{x} -> R)` (narrower width wins). |
| `function_compat` | The dispatch-time check that a `KObject::KFunction` value fills a typed function-shaped slot. | **Function subtyping** — contravariant params (width + depth) + covariant return. A value `(x :Any) -> Str` fills a slot typed `:(FN :{x :Number} -> Str)`; a value `(x :Number) -> Number` fills `:(FN :{x :Number} -> Any)`; a unary value fills a binary slot (the extra slot param arrives unbound under call-by-name). A value requiring a param the slot doesn't promise is a non-match. |

Admission (`function_compat`) and specificity (`is_more_specific_than`) share
**one** relation for function slots — contravariant params with width-subset,
covariant return — so most-specific-wins is consistent: the same value can now
fill several function slots at once (e.g. an `(x :Any) -> R` value fills both
`:(FN :{x :Number} -> R)` and `:(FN :{x :Any} -> R)`), and the ranking orders
those slots the same way admission does. Where one admitting slot is strictly
more specific than the others it wins outright; where two admitting slots are
genuinely incomparable — an `(x :Any) -> R` value against both
`:(FN :{x :Number} -> R)` and `:(FN :{x :Str} -> R)`, neither more specific —
dispatch ties and surfaces `AmbiguousDispatch`. The `List` / `Dict` covariance
is observable the same way: `(xs :(LIST OF Number))` strictly outranks
`(xs :(LIST OF Any))` for a number-list call.

**Return admission splits on whether the value's return is resolved or
deferred.** A `Resolved` value return admits covariantly as above — `sig_ret ==
ret || sig_ret ≺ ret`. A *deferred* value return (a per-call-elaborated
return like `-> :(TYPE OF er)`) carries no resolved `KType`, so `function_compat` admits it
by **syntactic equality of its surface shadow**: an `Any` slot admits any
deferred return; a slot whose `ret` is a `KType::DeferredReturn` carrier admits
iff its `DeferredReturnSurface` shadow equals the candidate's; any resolved slot
rejects, since a deferred return is opaque until per-call elaboration and refines
nothing more precise than its own shadow. The specificity short-circuit
`DeferredReturn ≺ Any` (covariant, via the `Any` arm) keeps a deferred-return
slot strictly more specific than an `Any`-return one.

**Record values subtype the dual way to function params.** A record value is
ranked by `record_value_more_specific`
([ktype_predicates.rs](../../../src/machine/model/types/ktype_predicates.rs)): a
*wider* record is **more specific** — a `{x = 1, y = "a"}` value (carried type
`:{x :Number, y :Str}`) fills a narrower `:{x :Number}` slot by dropping `y`, so the
superset arm wins a dispatch tie. Depth is **covariant** in the field types
(`:{x :Number}` ≺ `:{x :Any}`), sound because koan values are immutable
([memory-model](../../memory-model.md)). The relation is the dual of
`param_record_more_specific` (contravariant params with width-*drop* for
call-by-name) — records and function params share the `Record` substrate but order
opposite ways, so the two helpers stay separate. Incomparable record arms
(`:{x :Number, y :Str}` vs `:{x :Number, z :Str}`, filled by a value carrying all of
`x`, `y`, `z`) tie as `AmbiguousDispatch`; the [`FROM` projection
builtin](../../../src/builtins/record_projection.rs) breaks the tie at the call site —
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
FN (USE f :(FN :{x :Number} -> Str)) -> Str = ("got fn")

USE (FN (SHOW x :Number) -> Str = ("hi"))   # → "got fn"   (function_compat: equal by name+type)
USE (FN (SHOW x :Any)    -> Str = ("hi"))   # → "got fn"   (contravariant param: a value
                                            #   accepting Any fills a slot promising only Number)
```

```
FN (USE f :(FN :{x :Number, y :Str} -> Str)) -> Str = ("got fn")

USE (FN (SHOW x :Number) -> Str = ("hi"))   # → "got fn"   (width drop: a unary value fills a
                                            #   binary slot; the extra slot param `y` arrives
                                            #   unbound under call-by-name)
```

**Element-type inference for literals** is the join of element types via
[`TypeRegistry::join_iter`](../../../src/machine/model/types/registry.rs), computed
**once at construction** and memoized on the value's carrier: `[1, 2, 3]` →
`List<Number>`, `[1, "x"]` → `List<Any>`. `KObject::List` and `KObject::Dict`
each carry their element types directly (`List(&ListSubstrate, KType)` — a
region-resident element substrate plus a plain `Copy` `KType` handle — and
`Dict(&DictSubstrate, KType)`, a region-resident entry substrate plus the single
interned `Dict<key, value>` handle), so
[`KObject::ktype`](../../../src/machine/model/values/kobject.rs) reads the carried
type in O(1) rather than re-walking the contents on every call. Values are
immutable after construction, so the join is sound to compute exactly once. Functions project
their declared signature (`KObject::KFunction(f)` → `KFunction { params, ret }`,
the parameter record read off `f.signature`'s named slots). `TypeRegistry::join` joins
two same-shape `KFunction`s name-keyed — returns join, and parameters **meet**, because the
parameter position is contravariant and only their greatest lower bound is admitted by both
operands — coarsening a mismatched parameter-name set to `Any`.

### The lattice bottom

`Never` is the uninhabited bottom of the type lattice: admitted by no value, more specific
than every other type, and the identity element of both join and union canonicalization
(`:(Never | Number)` is `:Number`). It is spellable as a builtin type name, where `:Never`
declares a slot nothing fills. Its dual to `Any` makes the lattice complete, so
[`TypeRegistry::meet`](../../../src/machine/model/types/registry.rs) — the greatest lower
bound — is **total**: containers meet pointwise, records meet by field union (record values
are width-superset subtypes), unions distribute, functions meet by joining their parameters
and meeting their return, and a pair with no common refinement meets at `Never`.

**An empty container carries the bottom element type.** The join over no elements is the
join's identity, so `[]` memoizes `List<Never>` and an empty dict `Dict<Never, Never>`.
Element covariance then admits an empty container into *every* typed container slot with
nothing left to infer: `LET empty = []` binds, a bare top-level `[]` resolves, `TYPE OF []`
reports `:(LIST OF Never)`, and `[]` fills a `:(LIST OF Number)` parameter. (At the surface,
`{}` is the empty *record*, the top of the record lattice — an empty dict has no literal
spelling.) A *stamped* empty container (an `FN -> :(LIST OF Number) = ([])` whose carrier is
re-tagged to element `Number`) reports the stamped type; a heterogeneous non-empty literal
(`[2, "hello"]` → `List<Any>`) still carries `Any` and is legal only where `:(LIST OF Any)`
is declared.

### Runtime type-parameter carriers

`List`, `Dict`, and `Wrapped` carry their runtime type arguments on the variant so
dispatch and slot admission see the full instantiation, not just the outer shape:

- `KObject::List(items, list_type)` / `KObject::Dict(substrate, dict_type)` memoize the
  full interned container type handle at construction (`KObject::list` / `KObject::dict`),
  so `ktype()` is a handle copy.
- `KObject::Wrapped { type_id, .. }` carries the value's own type handle. When the applied
  type arguments are erased (the default from a projection construction and from `CATCH`),
  `type_id` is the bare `SetMember` handle; an ascription stamp populates it with a
  `ConstructorApply` over that member, folding the applied arguments into the one handle
  `ktype()` copies.

#### Applying a union head

A union head takes named type arguments — `:(Result {Ok = Number, Error = MyError})`, and
the same spelling over any user `UNION`. Each argument name must name a **member**, and the
application lands **per member**
([union.rs](../../../src/builtins/union.rs)): the result is the union of the members, each
named one replaced by the `ConstructorApply` over it carrying that one argument under the
member's own name, each unnamed one riding bare. Partial application is therefore legal on
a union head — a member no argument names simply stays unconstrained.

Per-member is the form both consumers can act on directly, because a value inhabits exactly
one member:

- **Admission.** The union slot admits a value any member admits, and the applied member
  hits the `ConstructorApply` arm of
  [ktype_predicates.rs](../../../src/machine/model/types/ktype_predicates.rs): the
  constructor must be the same, and then either a stamped `ConstructorApply` identity's
  arguments are checked per name against the declared args, or — for an erased carrier —
  the payload is checked against the argument the member's *own* name keys. A member's name
  and the argument's name agree by construction, so the linkage is a direct
  `arguments.get(member_name)` lookup with no separate ordering table, and a missing
  same-named argument admits.
- **Stamping.** `KObject::stamp_type` against a union node is the one declared shape that
  re-tags a carrier to something other than the declared handle itself: the `Wrapped` adopts
  the application over the member *it* inhabits, both sides peeled past any
  `ConstructorApply`. A member the slot left bare, and a value whose member the slot never
  declares, pass through unchanged.

The applied node is anonymous, so it renders structurally as the disjunction of its applied
members, like any other union node the surface did not name — the applied spelling does not
round-trip back through a bound name.

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
  [`finalize.rs`](../../../src/machine/execute/finalize.rs) then
  stamps the carrier to the declared type (`check_declared_return` →
  `KObject::stamp_type`). The **deferred**-return (`PerCall`) path checks only: the same
  [`check_declared_return`](../../../src/machine/execute/finalize.rs) runs
  the match predicate but returns no stamp, so a satisfying value passes through
  un-stamped (a passing value
  already satisfies the declared type, at worst as a subtype).
- **FN argument** — each parameterized-carrier argument slot (`List` / `Dict` /
  `ConstructorApply`) is checked with `matches_value` in
  [`KFunction::validate_call_args`](../../../src/machine/core/kfunction.rs) before the
  body binds — a uniquely-picked call is admitted shape-only by dispatch, so this is
  where a non-satisfying typed argument becomes a hard `TypeMismatch` rather than
  slipping through. The check is not followed by an argument stamp. This
  `matches_value` walk is the authoritative content-recursive check; for `List` /
  `Dict` it confirms what dispatch already gates, since an evaluated container whose
  carried element type doesn't satisfy the slot is rejected as a dispatch non-match
  (see [Dispatch and slot-specificity](dispatch.md#dispatch-and-slot-specificity)).
- **`LET`** ascription — same check-then-stamp on the bound value.

**Parameter arity is fixed by the keyworded sigil shape.** `:(LIST OF <Elem>)`
carries exactly one element slot and `:(MAP <Key> -> <Value>)` exactly two, so an
arity mismatch isn't expressible at the surface — the type-constructor
overloads only match the well-formed shape, and any other arrangement
fails to resolve as a parameterized type at all. A *declared* constructor family
instead applies by name (`:(Pair {Key = Number, Val = Str})`), where an arity or
name mismatch is a shape error naming the missing and unknown keys — see
[functors.md § Higher-kinded type slots](../functors.md#higher-kinded-type-slots).
(See
[elaboration.md § Layers](../elaboration.md#layers) § Layer 1 for where type
elaboration sits in the pipeline.)

`KFunction` is not a surface-declarable type name — there's no "any function"
KType, since a function with no signature has nothing to dispatch on. Use
`:(FN :{args…} -> R)` for typed shapes or `Any` for unconstrained values.
FN's own registered return type is `KType::Any` for the same reason: the
constructed function's projected `ktype()` carries its real shape at runtime.

