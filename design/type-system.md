# Type system

## Token classes — the parser-level foundation

The lexer ([tokens.rs](../src/parse/tokens.rs)) splits non-literal atoms into
three classes:

- **Keyword** — pure-symbol tokens (`=`, `->`, `:|`, `:!`, `+`) and alphabetic
  tokens with **two or more uppercase letters and no lowercase letters**
  (`LET`, `THEN`, `MODULE`, `SIG`). Contribute fixed tokens to a signature's
  bucket key. The two-uppercase floor reserves single-letter capitals (`A`,
  `K`) and uppercase-plus-digits shapes (`K9`, `AB1`) as syntactic territory
  rather than letting them silently classify as identifiers — see below.
- **Type** — uppercase-leading with at least one lowercase letter elsewhere
  (`Number`, `Str`, `KFunction`, `MyType`, `IntOrd`, `OrderedSig`). Type
  references, module names, and signature names all share this class.
- **Identifier** — lowercase-leading or `_`-leading names (`compare`,
  `my_var`, `_internal`).

This split is what lets the language reserve a syntactic slot for type names
without quoting. `FN (x: Number) -> Str = (...)` works because `Number` and
`Str` are recognizable as types from their shape alone.

A token that starts uppercase but classifies as neither keyword nor type
(e.g. a single uppercase letter `A`, or `K9`) is a parse error rather than
falling through to identifier — the rule keeps the type-position slot
syntactically discriminable and prevents a future binding from silently
shadowing a one-letter type-position identifier.

The [module system](module-system.md) reuses the Type class without adding
a fourth: module names (`IntOrd`, `MakeSet`) and signature names
(`OrderedSig`, `ShowableSig`) classify the same way as host type names. The
discrimination between "host type", "module", and "signature" happens at
scope resolution, not at lex time — a `.`-compound on a module-class token
resolves to module member access the same way a `.`-compound on a struct
value resolves to a field read, and a module-qualified `IntOrd.Type` in type
position parses as a single structured `TypeExpr`. Abstract type
declarations inside a signature use the Type-class spelling too — the
convention is `LET Type = ...` for the principal abstract type, with `Elt`,
`Key`, `Val` etc. when more than one is needed.

## `KType` — the runtime type system

[`KType`](../src/runtime/model/types/ktype.rs) has a variant for every concrete `KObject`:

- Scalars: `Number`, `Str`, `Bool`, `Null`.
- Containers: `List(Box<KType>)`, `Dict(Box<KType>, Box<KType>)`,
  `KFunction { args: Vec<KType>, ret: Box<KType> }`. Always parameterized; see
  [the next section](#container-type-parameterization).
- Other function-like: `KExpression` (a captured-but-unevaluated expression).
- Meta-type for type-position slots: `TypeExprRef` — see
  [Type-position slot kinds](#type-position-slot-kinds).
- First-class type values: `Type` (a tagged-union or struct schema), `Tagged`
  (a tagged-union variant value), `Struct` (a struct value).
- Module-system carriers: `Module` (the type of a `MODULE` value),
  `Signature` (the type of a `SIG` value), and
  `ModuleType { scope_id: usize, name: String }` — the per-ascription
  abstract-type carrier minted by `:|` opaque ascription. Two distinct
  opaque ascriptions of the same source module mint distinct `ModuleType`s
  (different `scope_id`s), giving the abstraction-barrier identity property
  the [module system](module-system.md) rests on.
- `Any` — the no-op fast-path.

[`KType::matches_value`](../src/runtime/model/types/ktype_predicates.rs) plus
[`KObject::ktype`](../src/runtime/model/values/kobject.rs) close the loop on runtime
checking: every value has a queryable type, and any declared type can be checked
against it.

## Container type parameterization

`List<T>`, `Dict<K, V>`, and `Function<(args) -> ret>` carry their inner types
on the variant directly. `KType` is not `Copy`; structural payloads are
`Box`ed where the variant would otherwise be self-referential.

**Surface syntax** is angle brackets. The parser treats `<...>` as an intratoken
group anchored to a preceding type identifier — `List<Number>` is one
[`ExpressionPart::Type`](../src/ast.rs) carrying a structured
`TypeExpr`, not three tokens. A bare `<` or `>` outside that context (e.g.,
`a < b` with whitespace) flows through as a `Keyword`, so a future less-than
builtin is unblocked. The framing logic lives in
[type_frame.rs](../src/parse/type_frame.rs).

### Variance

Variance is split across the parameterized constructors. `List` and `Dict` are
covariant in their parameter positions; `Function` is invariant in args and
return. The split falls out of the underlying check in each case rather than
being a deliberate design dial — both choices are the natural one given how
the constructor's values are matched, and the conservative `Function`-invariant
rule keeps dispatch unambiguous.

Three sites consume parameterized types, and each has its own behavior:

| Site | What it does | Variance |
|---|---|---|
| `matches_value` | Walks a runtime value against a declared type at the return-type check. | **Covariant** for `List` / `Dict`: `List<Any>` accepts any list because `Any.matches_value(_)` is always true; `Dict<Str, Any>` accepts a `{a: 1, b: "x"}` value. **Invariant** for `Function`: delegates to `function_compat`. |
| `is_more_specific_than` | Ranks two slot types when multiple overloads match the same call. Used by `specificity_vs` to break dispatch ties. | **Covariant in every parameter position** (element, key, value, arg, ret): `List<Number>` ≺ `List<Any>`, `Dict<Str, Number>` ≺ `Dict<Str, Any>`, `Function<(Number) -> Str>` ≺ `Function<(Any) -> Any>`. |
| `function_compat` | The dispatch-time check that a `KObject::KFunction` value fills a typed function-shaped slot. | **Strict structural equality** — invariant. A function declared `(x: Number) -> Str` fills only `Function<(Number) -> Str>`, not `Function<(Any) -> Str>`. |

The combination is sound for dispatch even though `is_more_specific_than`
ranks `Function`-typed slots covariantly while `function_compat` is invariant.
The covariant ranking only matters when two parameterized function slots both
match the same call; with `function_compat`'s strict equality, a function
value matches at most one parameterized function slot, so the ranking has no
tie to break in that case. The covariance is observable for `List` / `Dict`
tournaments — `(xs: List<Number>)` strictly outranks `(xs: List<Any>)` for a
number-list call — and benign for `Function`.

Concretely:

```
LET nums = [1 2 3]

FN (PICK xs: List<Any>)    -> Str = ("any")
FN (PICK xs: List<Number>) -> Str = ("number")

PICK nums   # → "number"   (covariant: List<Number> ≺ List<Any>)
```

```
FN (BAD) -> List<Number> = ([1 "x"])
BAD   # → TypeMismatch: expected List<Number>, got List<Any>
        # (matches_value walks elements; covariant — Any.matches_value(_) is true,
        #  Number.matches_value("x") is false)
```

```
FN (USE f: Function<(Number) -> Str>) -> Str = ("got fn")

USE (FN (SHOW x: Number) -> Str = ("hi"))   # → "got fn"   (function_compat: equal)
USE (FN (SHOW x: Any)    -> Str = ("hi"))   # → DispatchFailed
                                            #   (function_compat: invariant, not equal)
```

**Element-type inference for literals** is the join of element types via
[`KType::join_iter`](../src/runtime/model/types/ktype_resolution.rs): `[1, 2, 3]` → `List<Number>`,
`[1, "x"]` → `List<Any>`, `[]` → `List<Any>`.
[`KObject::ktype`](../src/runtime/model/values/kobject.rs) walks list elements and dict
keys/values on each call to project the parameterized form; functions project
their declared signature (`KObject::KFunction(f, _)` → `KFunction { args, ret }`
read off `f.signature`).

**Element validation runs on returns, not arguments.** The scheduler's
runtime return-type check walks `matches_value` over the returned value,
recursing into containers (a list literal `[1, "x"]` returned where
`List<Number>` was declared fails with a structured `TypeMismatch` naming both
types). Argument-position element validation is shape-only at dispatch — an
`[x, y]` literal with sub-expression elements can't be type-checked until the
elements evaluate. See open work for the static-pass-driven closure of this
gap.

**Arity is enforced at FN-definition time** by `KType::from_type_expr`:
`List<A, B>` rejects with a precise error before the function is ever called.

`KFunction` is no longer a surface-declarable type name — there's no
"any function" KType, since a function with no signature has nothing to
dispatch on. Use `Function<(args) -> R>` for typed shapes or `Any` for
unconstrained values. FN's own registered return type is `KType::Any` for the
same reason: the constructed function's projected `ktype()` carries its real
shape at runtime.

## Type-position slot kinds

`TypeExprRef` is the meta-type for argument slots that capture a parsed type-name
token (`ExpressionPart::Type(_)`). The slot resolves to a
`KObject::KTypeValue(KType)` carrying the elaborated type — name, nested
parameters, and (for recursive types) `Mu` / `RecursiveRef` structure — so
parameterized types like `List<Number>` and recursive types like `Tree`
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
[`KErrorKind::DispatchFailed`](../src/runtime/machine/core/kerror.rs); the same call shape
with different parameter types routes to a different overload by
slot-specificity (see below).

The return type is non-optional and runtime-enforced. The scheduler injects a
check at user-fn slot finalization that surfaces
[`KErrorKind::TypeMismatch`](../src/runtime/machine/core/kerror.rs) (with a `<return>` arg
name and a frame naming the called function) on mismatch. `Any` is the
no-enforcement fast path for sites that genuinely don't care.

FN itself registers with a return type of `Any` — there's no "any function"
KType to declare, since a function with no signature has nothing to dispatch
on; the constructed function's projected `ktype()` carries the real shape at
runtime.

## Type elaboration

Type elaboration runs in the same scheduler that runs value evaluation.
A type-binding site (`LET T = ...`, `STRUCT T = ...`, `UNION T = ...`)
registers a placeholder in the
[`Bindings`](../src/runtime/machine/core/scope.rs) façade on `Scope` — the
same `placeholders` table value bindings use, sitting alongside `data` and
`functions` — and dispatches its body as scheduler work.
Lookups of type names from outside the body park on the producer's NodeId
via `notify_list` / `pending_deps`, the same path value-name forward
references take ([execution-model.md § Dispatch-time name placeholders](execution-model.md#dispatch-time-name-placeholders)).
This makes type-name and value-name forward references compose uniformly:
submission order is not load-bearing for either.

**Recursion via threaded-set self-reference recognition.** The elaborator
threads a set of binder names currently being elaborated. A lookup of a
name in the set returns `KType::RecursiveRef(name)` directly without
parking — this is what keeps a recursive type definition from deadlocking
on its own placeholder. At binding finalization, if any self-reference
fired, the body wraps in `KType::Mu { binder, body }`; otherwise it
commits bare. There is no transient `KType::Placeholder` variant —
recognition lives in the elaborator's call frame, not in the type
language. Mutual recursion seeds the threaded set with all names in a
strongly-connected declaration group before elaborating any member's
body, so `STRUCT TreeA { b: TreeB }` and `STRUCT TreeB { a: TreeA }`
elaborate as a unit with cross-references becoming `RecursiveRef` directly.

**Why threaded-set rather than a tagged placeholder or NodeId sentinel.**
Threading the set keeps recursion recognition layered above the scheduler:
the scheduler stays type-agnostic (no awareness of "who's elaborating
right now"), the type language stays scheduler-agnostic (no NodeIds
embedded in `KType`), and recursion is purely the elaborator's concern.
SCC mutual recursion just expands the set. Tagging the scheduler
placeholder with "currently elaborating by node N" couples NodeIds into
the type language; sentinel-by-NodeId smuggles runtime identity into
`KType` during the elaboration window. Both alternatives violate the
layering this design preserves.

**One canonical runtime type representation.** Type bindings finalize to
`KObject::KTypeValue(KType)`. Consumers read the elaborated type
directly; there is no surface/elaborated split, no per-lookup
re-elaboration, no parallel `TypeExpr` representation flowing through
dispatch. Cycle-aware traversals (equality, printing, hashing) carry an
"inside this `Mu` binder" set so back-references terminate after one
unfold. Trivially cyclic aliases (`LET T = T`) surface as a structured
error rather than a stack overflow.

## Dispatch and slot-specificity

When multiple registered functions match an incoming expression, dispatch picks
by slot-specificity: typed slots outrank untyped ones; literal-typed slots
outrank `Any`. See [expressions-and-parsing.md](expressions-and-parsing.md) for
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

The static-typing-and-jit work in open work closes the first two uniformly.

## Open work

The abstraction-over-types story is the [module
system](module-system.md) — structures and signatures, opaque ascription as
the type-abstraction primitive, functors for parametric types, and modular
implicits for inferred dispatch. Stage 1 (the module language and per-module
type identity via `KType::ModuleType`) shipped and is described in the body
above; the remaining stages live under
[`roadmap/module-system-*.md`](../roadmap/module-system-2-scheduler.md).

- [Eager type elaboration with placeholder-based recursion](../roadmap/eager-type-elaboration.md)
  — closes the remaining gaps in the elaboration mechanism described
  above: SCC pre-registration so mutually recursive STRUCT/UNION groups
  elaborate without deadlocking on each other's placeholders,
  parens-wrapped FN-parameter sub-dispatch (`xs: (LIST_OF MyType)`),
  `OnceCell<KType>` late binding for signature-typed parameters whose
  type only resolves at functor application time, and the phase-5
  cleanup that retires `NoopResolver` and the transitional
  `KType::Unresolved` carrier.
- [Module system stage 5 — Modular implicits](../roadmap/module-system-5-modular-implicits.md)
  — inferred dispatch on signatures. Lands the multi-parameter dispatch the
  current slot-specificity ranking can't express on its own.
- [Group-based operators](../roadmap/group-based-operators.md) — paired
  operators like `+`/`-` as a single algebraic declaration. Lands on top of
  the module-system substrate.
- [Static type checking and JIT compilation](../roadmap/static-typing-and-jit.md)
  — closes the TCO and builtin runtime-check gaps uniformly, and is the
  language's performance ceiling. The module system's compile-time
  scheduling — type-returning builtins dispatched and bound through the
  same `Dispatch`/`Bind` machinery values use, with stage 5 implicit
  search layered as a `SEARCH_IMPLICIT` builtin — is the substrate this
  work builds on.
