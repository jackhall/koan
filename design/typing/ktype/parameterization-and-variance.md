# Parameterization, variance, and runtime carriers

Container type parameterization, the variance lattice that orders slot
specificity, and the runtime carriers for type parameters. Part of the
[`KType` reference](README.md).

## Container type parameterization

`:(LIST OF T)`, `:(MAP K -> V)`, and `:(FN (args) -> ret)` carry their inner
types on the variant directly. `KType` is not `Copy`; structural payloads are
`Box`ed where the variant would otherwise be self-referential.

**Surface syntax** is a glued-right `:` sigil opening an S-expression
type-expression group. The parser treats `:(...)` as a parse-context marker
anchored to the `:` — a `:(...)` sigil emits one
[`ExpressionPart::SigiledTypeExpr(Box<KExpression>)`](../../../src/machine/model/ast.rs)
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
([dispatch.rs](../../../src/machine/execute/dispatch.rs))
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
| `is_more_specific_than` | Ranks two slot types when multiple overloads match the same call. Used by `specificity_vs` to break dispatch ties. Concrete carrier types also outrank the unconstrained-name slot types `Identifier` and `OfKind(Proper)`, so a concrete-typed `ATTR` overload beats an `ATTR <s:Identifier>` fallback when both admit. | **Covariant** for `List` / `Dict` (element, key, value): `:(LIST OF Number)` ≺ `:(LIST OF Any)`, `:(MAP Str -> Number)` ≺ `:(MAP Str -> Any)`. **Contravariant params (with width-subset) + covariant return** for `Function`, matching `function_compat`: `:(FN (x :Any) -> Str)` ≺ `:(FN (x :Number) -> Str)` (more-general param wins), `:(FN (x) -> Number)` ≺ `:(FN (x) -> Any)` (narrower return wins), and a nullary `:(FN () -> R)` ≺ a unary `:(FN (x) -> R)` (narrower width wins). |
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
[`KType::join_iter`](../../../src/machine/model/types/ktype_resolution.rs), computed
**once at construction** and memoized on the value's carrier: `[1, 2, 3]` →
`List<Number>`, `[1, "x"]` → `List<Any>`. `KObject::List` and `KObject::Dict`
each carry their element types directly (`List(Rc<Vec<…>>, Box<KType>)`,
`Dict(…, Box<KType>, Box<KType>)`), so
[`KObject::ktype`](../../../src/machine/model/values/kobject.rs) reads the carried
type in O(1) rather than re-walking the contents on every call. Values are
immutable `Rc`, so the join is sound to compute exactly once. Functions project
their declared signature (`KObject::KFunction(f, _)` → `KFunction { params, ret }`,
the parameter record read off `f.signature`'s named slots). `KType::join` joins
two same-shape `KFunction`s name-keyed, coarsening a
mismatched parameter-name set to `Any`.

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
  parameterized union as a `Record<KType>` keyed by the carrier's *parameter names*
  (`Result` binds `Ok` and `Error`). Empty `type_args` means erased — `ktype()`
  reports the bare `SetRef`; a populated carrier makes `ktype()`
  synthesize `ConstructorApply { ctor, args: type_args }`. Construction
  (`tagged_union::construct`, `CATCH`) erases by default; the carrier is populated
  only by ascription stamping.

A `ConstructorApply` slot (`:(Result {Ok = Number, Error = MyError})`) admits a
`Tagged` value via the `matches_value` arm in
[ktype_predicates.rs](../../../src/machine/model/types/ktype_predicates.rs): the
declaring schema must be the same constructor, and then either the populated
`type_args` are checked per parameter name against the declared args, or — for an
erased carrier — the *inhabited* tag's payload is checked against the same-named
argument. `Result`'s tag names and its parameter names coincide by construction
(`Ok`, `Error`), so the field→parameter linkage is a direct `args.get(tag)` lookup
with no separate ordering table.

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
`:(FN (args) -> R)` for typed shapes or `Any` for unconstrained values.
FN's own registered return type is `KType::Any` for the same reason: the
constructed function's projected `ktype()` carries its real shape at runtime.

