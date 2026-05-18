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
  reported by `KObject::StructType` and `KObject::TaggedUnionType`).
- User-declared nominal types: `UserType { kind: UserTypeKind, scope_id: usize,
  name: String }` — the per-declaration identity tag synthesized by
  `KObject::ktype()` for `Struct`, `Tagged`, and `KModule` carriers and minted
  by `:|` opaque ascription with `kind: Module`. Two distinct STRUCTs (or two
  distinct opaque ascriptions of the same source module) produce different
  `scope_id`s, giving the abstraction-barrier and per-declaration-distinctness
  identity property the [module system](modules.md) rests on.
  `UserTypeKind` is `Struct | Tagged | Module | Newtype { repr } |
  TypeConstructor { param_names }`. The two payload-carrying variants
  (`Newtype`, `TypeConstructor`) have a manual `PartialEq` that ignores their
  payloads — identity equality is by variant tag plus the carrier's
  `(scope_id, name)`, so wildcard / concrete pairs compare equal.
  The companion `AnyUserType { kind }` wildcard accepts any `UserType` of the
  matching kind, used for slot types that admit "any user-declared X" — ATTR's
  `body_struct` / `body_module` slots, `MODULE`'s declaration slot, `:|` / `:!`
  ascription, construction primitives' return types. The surface keywords
  `Newtype` and `TypeConstructor` are pinned for diagnostic rendering but not
  registered as writable surface names (no entry in
  [`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs)).
- Higher-kinded application: `ConstructorApply { ctor: Box<KType>, args:
  Vec<KType> }` — structural identity by `(ctor, args)`, mirror of `List(_)`
  / `Dict(_, _)`. Emitted by `elaborate_type_expr` when the outer name of a
  parameterized `TypeExpr` resolves to a
  `UserType { kind: TypeConstructor { .. }, .. }`; renders as `ctor<arg1,
  arg2>` in diagnostics. See
  [functors.md § Higher-kinded type slots](functors.md#higher-kinded-type-slots)
  for the surface form and per-call generativity.
- Signature carriers: `Signature` (the type of a first-class `SIG` value) and
  `SignatureBound { sig_id, sig_path }` — the per-declaration `SIG` identity
  written into `bindings.types` at finalize time so signature names resolve
  uniformly through `Scope::resolve_type`.
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
| `matches_value` | Walks a runtime value against a declared type at the return-type check. | **Covariant** for `List` / `Dict`: `:(List Any)` accepts any list because `Any.matches_value(_)` is always true; `:(Dict Str Any)` accepts a `{a: 1, b: "x"}` value. **Invariant** for `Function`: delegates to `function_compat`. |
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
[`KType::join_iter`](../../src/machine/model/types/ktype_resolution.rs): `[1, 2, 3]` → `:(List Number)`,
`[1, "x"]` → `:(List Any)`, `[]` → `:(List Any)`.
[`KObject::ktype`](../../src/machine/model/values/kobject.rs) walks list elements and dict
keys/values on each call to project the parameterized form; functions project
their declared signature (`KObject::KFunction(f, _)` → `KFunction { args, ret }`
read off `f.signature`).

**Element validation runs on returns, not arguments.** The scheduler's
runtime return-type check walks `matches_value` over the returned value,
recursing into containers (a list literal `[1, "x"]` returned where
`:(List Number)` was declared fails with a structured `TypeMismatch` naming both
types). Argument-position element validation is shape-only at dispatch — an
`[x, y]` literal with sub-expression elements can't be type-checked until the
elements evaluate. See [Known limitations](#known-limitations) for the
static-pass-driven closure of this gap.

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

## Known limitations

- **TCO collapses frames.** When A tail-calls B, only B's return type is
  checked at runtime — the slot's `function` field is replaced at TCO time.
- **Builtins are not runtime-checked.** They return through `BodyResult::Value`
  with no slot frame, so the runtime check has nowhere to attach. Their
  declared return types are honest but unenforced.
- **Argument-position element validation is shape-only.** Container slots
  accept any list/dict at dispatch; element types are checked only on
  returns. Lifting this needs literal-element peeking for fully-literal
  collections plus a deferred check for sub-expression elements.

The two-phase execution work in [open-work.md](open-work.md) closes the first
two uniformly.
