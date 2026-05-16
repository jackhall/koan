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
without quoting. `FN (x :Number) -> Str = (...)` works because `Number` and
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

[`KType`](../src/runtime/machine/model/types/ktype.rs) has a variant for every concrete `KObject`:

- Scalars: `Number`, `Str`, `Bool`, `Null`.
- Containers: `List(Box<KType>)`, `Dict(Box<KType>, Box<KType>)`,
  `KFunction { args: Vec<KType>, ret: Box<KType> }`. Always parameterized; see
  [the next section](#container-type-parameterization).
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
  identity property the [module system](module-system.md) rests on.
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
  [`KType::from_name`](../src/runtime/machine/model/types/ktype_resolution.rs)).
- Higher-kinded application: `ConstructorApply { ctor: Box<KType>, args:
  Vec<KType> }` — structural identity by `(ctor, args)`, mirror of `List(_)`
  / `Dict(_, _)`. Emitted by `elaborate_type_expr` when the outer name of a
  parameterized `TypeExpr` resolves to a
  `UserType { kind: TypeConstructor { .. }, .. }`; renders as `ctor<arg1,
  arg2>` in diagnostics. See
  [module-system.md § Higher-kinded type slots](module-system.md#higher-kinded-type-slots)
  for the surface form and per-call generativity.
- Signature carriers: `Signature` (the type of a first-class `SIG` value) and
  `SignatureBound { sig_id, sig_path }` — the per-declaration `SIG` identity
  written into `bindings.types` at finalize time so signature names resolve
  uniformly through `Scope::resolve_type`.
- `Any` — the no-op fast-path.

[`KType::matches_value`](../src/runtime/machine/model/types/ktype_predicates.rs) plus
[`KObject::ktype`](../src/runtime/machine/model/values/kobject.rs) close the loop on runtime
checking: every value has a queryable type, and any declared type can be checked
against it.

## Container type parameterization

`:(List T)`, `:(Dict K V)`, and `:(Function (args) -> ret)` carry their inner
types on the variant directly. `KType` is not `Copy`; structural payloads are
`Box`ed where the variant would otherwise be self-referential.

**Surface syntax** is a glued-right `:` sigil opening an S-expression
type-expression group. The parser treats `:(...)` as a type-position frame
anchored to the `:` — `:(List Number)` is one
[`ExpressionPart::Type`](../src/runtime/machine/model/ast.rs) carrying a structured
`TypeExpr`, not four tokens. `<` and `>` flow through unencumbered as
keyword tokens, leaving the arithmetic comparison operators available. The
framing logic lives in
[type_expr_frame.rs](../src/parse/type_expr_frame.rs).

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
[`KType::join_iter`](../src/runtime/machine/model/types/ktype_resolution.rs): `[1, 2, 3]` → `:(List Number)`,
`[1, "x"]` → `:(List Any)`, `[]` → `:(List Any)`.
[`KObject::ktype`](../src/runtime/machine/model/values/kobject.rs) walks list elements and dict
keys/values on each call to project the parameterized form; functions project
their declared signature (`KObject::KFunction(f, _)` → `KFunction { args, ret }`
read off `f.signature`).

**Element validation runs on returns, not arguments.** The scheduler's
runtime return-type check walks `matches_value` over the returned value,
recursing into containers (a list literal `[1, "x"]` returned where
`:(List Number)` was declared fails with a structured `TypeMismatch` naming both
types). Argument-position element validation is shape-only at dispatch — an
`[x, y]` literal with sub-expression elements can't be type-checked until the
elements evaluate. See open work for the static-pass-driven closure of this
gap.

**Arity is enforced at FN-definition time** by `KType::from_type_expr`:
`:(List A B)` rejects with a precise error before the function is ever called.

`KFunction` is no longer a surface-declarable type name — there's no
"any function" KType, since a function with no signature has nothing to
dispatch on. Use `:(Function (args) -> R)` for typed shapes or `Any` for
unconstrained values. FN's own registered return type is `KType::Any` for the
same reason: the constructed function's projected `ktype()` carries its real
shape at runtime.

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
A type-binding site (`LET Ty = ...`, `STRUCT Ty = ...`, `UNION Ty = ...`)
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
unfold. Trivially cyclic aliases (`LET Ty = Ty`) surface as a structured
error rather than a stack overflow.

**Module-qualified type names.** A TypeExpr like `Mo.Ty` or chained
`Outer.Inner.T` resolves through the value-side ATTR walker:
[`access_module_member`](../src/runtime/builtins/attr.rs) tries the
module's `type_members` table (opaque-ascription type bindings), then
the child scope's `data` (so chained `Outer.Inner.X` reads the inner
*module value* and the chain stays drillable), then the child scope's
type-side `bindings.types` via `Scope::resolve_type` — synthesizing a
`KTypeValue` carrier so type-position consumers (e.g. a LET-RHS routing
through Combine) see a first-class `KType` value. The resolved type is
the leaf's existing per-declaration `KType::UserType { kind, scope_id,
name }`; no new variant, no path field.

**Non-SCC forward type aliases.** A top-level `LET Ty = Un; LET Un = Number`
binds without rejection: the Type-classed `Un` token on the first LET's
RHS parks on the producer's dispatch-time placeholder via the same
mechanism value-name forward references use, resumes when `LET Un =
Number` finalizes, and writes through `Scope::register_type` to land in
`bindings.types`. The mutual-recursion SCC sweep covers the cycle case;
the placeholder-park rail covers the source-order case.

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
implicits for inferred dispatch. Stages 1 and 2 (the module language,
per-module type identity via `KType::UserType { kind: Module, .. }`, and
the scheduler-driven elaborator plus `SIG_WITH` sharing constraints and
higher-kinded type-constructor slots) shipped and are described in the
body above; the remaining stages live under
[`roadmap/module-system-*.md`](../roadmap/module-system-4-axioms-and-generators.md).

Per-call FN-parameter substitution into return-type pins — the templated
return-type surface described in
[module-system.md § Functors](module-system.md#functors) — rides the
`ReturnType` / `DeferredReturn` carriers at
[`ExpressionSignature::return_type`](../src/runtime/machine/model/types/signature.rs)
and the dispatch-boundary re-elaboration in
[`KFunction::invoke`](../src/runtime/machine/core/kfunction/invoke.rs);
together with the shipped identifier-class hardening at the bare-leaf
arm of `elaborate_type_expr`, it completes the surface
"modules-as-types" presentation.

The four-stage type-identity arc routes bare-leaf type names through a
`KObject`-side carrier rather than a placeholder variant inside the
elaborated type language, unifies STRUCT / UNION / MODULE / opaque-ascription
identity onto a single `KType::UserType { kind, scope_id, name }` carrier
with per-declaration scope-tagged identity, and adds the `NEWTYPE` keyword
for fresh nominal identity over a transparent representation. The binding
home splits into two maps (`data` for values, `types` for type names), with
token-kind-driven lookup at the resolver — Type-class tokens consult
`types`, identifier tokens consult `data`. All four stages have shipped.

- Type identity stage 1 — foundation: dual-map binding home,
  `Scope::resolve_type`, dispatch routing by token kind, bind-time
  diagnostic for type-class LHS with non-type RHS, and LET routing of
  Type-class aliases through `register_type`. The arena slot
  ([`RuntimeArena::alloc_ktype`](../src/runtime/machine/core/arena.rs)),
  the
  [`Bindings::types` map plus `try_register_type` and `try_register_nominal` write primitives](../src/runtime/machine/core/bindings.rs),
  the
  [`Scope::register_type` rewire onto `bindings.types` plus the type-side `Scope::resolve_type` lookup API](../src/runtime/machine/core/scope.rs),
  the
  [consumer migration onto `Scope::resolve_type` with `Scope::resolve`'s transient fallback deleted](../src/runtime/builtins/value_lookup.rs),
  the
  [`KErrorKind::TypeClassBindingExpectsType` bind-time diagnostic in `let_binding`](../src/runtime/builtins/let_binding.rs)
  that rejects `LET <Type-class> = <non-type>` at the binder, and the
  [LET `TypeExprRef`-LHS routing through `register_type`](../src/runtime/builtins/let_binding.rs)
  for type-valued RHSes have landed. Builtin type names *and*
  `LET Ty = Number`-style aliases live in `bindings.types` as
  arena-allocated `&KType`, reachable through `Scope::resolve_type` on
  the same pointer as the builtin. Type-token reads (the `TypeExprRef`
  overload of `value_lookup` and the bare-leaf arm of
  `elaborate_type_expr`) consult `Scope::resolve_type` first; the sole
  `KObject::KTypeValue` synthesis site for dispatch transport lives in
  [`value_lookup::body_type_expr`](../src/runtime/builtins/value_lookup.rs),
  which mints `KObject::KTypeValue(kt.clone())` on a `resolve_type` hit.
  On a `resolve_type` miss, the bare-leaf arm of `elaborate_type_expr`
  falls through to `Scope::resolve` for compatibility with the small set of
  callers that still consult the value side; the `body_type_expr` reader,
  by contrast, is types-only. Value-side nominal carriers —
  `KObject::KModule` from `MODULE`, `KObject::StructType` from `STRUCT`,
  `KObject::TaggedUnionType` from `UNION`, `KObject::KSignature` from
  `SIG` — are dual-written into `bindings.types` next to a
  `KType::UserType` or `KType::SignatureBound` by the finalize routes. The bind-time check uses a
  primitive/container blocklist
  (`Number | Str | Bool | Null | List(_) | Dict(_, _)`) so type-language
  carriers (`KModule`, `KSignature`, `StructType`, `TaggedUnionType`),
  whose runtime `KType` is `Module` / `Signature` / `Type` rather than
  `TypeExprRef`, continue to bind through the existing `bind_value` path.
  Ascription's abstract-type member sweep
  ([`ascribe.rs`](../src/runtime/builtins/ascribe.rs)) walks both
  `bindings.types` and `bindings.data` via the `abstract_type_names_of`
  helper, so SIG `Type` declarations resolve uniformly whether the
  signature body's LET wrote to `types` (Type-class LHS, `KTypeValue`
  RHS) or to `data` (other type-language carriers).
- Type identity stage 2 — `KObject::TypeNameRef` carrier. Bare-leaf type
  names that aren't in
  [`KType::from_name`](../src/runtime/machine/model/types/ktype.rs)'s builtin table
  (`Point`, `IntOrd`, `MyList`) are lowered by
  [`ExpressionPart::resolve_for`](../src/runtime/machine/model/ast.rs) into
  `KObject::TypeNameRef(TypeExpr)` rather than a placeholder `KType`
  variant. The carrier preserves the parser-side `TypeExpr` for diagnostics
  and for consumers that want the user's surface identifier verbatim; it
  carries no internal cache. Both `TypeNameRef` and the fully-resolved
  `KTypeValue` report `ktype() = KType::TypeExprRef`, so the slot's
  dispatch position is identical — whether the surface form already lowered
  to a concrete `KType` at bind time or is still in parser-form is an
  internal detail. The four downstream consumers each carry a
  `TypeNameRef` arm beside the existing `KTypeValue` arm: the shared
  [`extract_bare_type_name`](../src/runtime/machine/core/kfunction/argument_bundle.rs)
  helper (used by STRUCT/UNION declaration sites and `type_call`'s verb
  slot), [ATTR's `body_type_lhs` and `read_field_name`](../src/runtime/builtins/attr.rs),
  [`let_binding`'s name slot](../src/runtime/builtins/let_binding.rs) (which
  runs the same primitive/container blocklist as the `KTypeValue` arm and
  routes to `register_type` for type-valued RHSes), and
  [`value_lookup::body_type_expr`](../src/runtime/builtins/value_lookup.rs)
  (which resolves through `bindings.types` and, on a nominal `UserType` /
  `SignatureBound` hit, recovers the paired value-side carrier from
  `bindings.data`). FN's deferred return-type elaboration peeks the slot
  to pick between
  [`extract_ktype`](../src/runtime/machine/core/kfunction/argument_bundle.rs)
  (resolved carrier) and the sibling
  [`extract_type_name_ref`](../src/runtime/machine/core/kfunction/argument_bundle.rs)
  (deferred carrier consuming the parser-preserved `TypeExpr`), then drives
  the existing park-on-placeholder machinery from there.

  Two complementary caches amortize the elaboration cost rather than one
  cell on the carrier:

    * **Layer 1 — surface-form, scope-independent.** A
      `OnceCell<KType>` lives on
      [`TypeExpr`](../src/runtime/machine/model/ast.rs) itself
      (`TypeExpr.builtin_cache`, excluded from `PartialEq` / `Hash`).
      `ExpressionPart::resolve_for` reads the cell first; on miss it
      runs [`KType::from_type_expr`](../src/runtime/machine/model/types/ktype_resolution.rs)
      and writes the result back when the surface form resolves against
      the builtin table. `from_type_expr` failures (user-bound leaves)
      are intentionally not cached here — those flow through Layer 2.
      Subsequent `KFunction::bind` passes against the same `TypeExpr`
      pay one `OnceCell::get`. No scope keying needed: the cached
      `KType` depends solely on the surface form.

    * **Layer 2 — scope-bound resolution memo.** A
      `RefCell<HashMap<TypeExpr, &'a KType>>` lives on
      [`Bindings`](../src/runtime/machine/core/bindings.rs)
      (`type_expr_memo`). Reached through
      [`Scope::resolve_type_expr`](../src/runtime/machine/core/scope.rs),
      which returns the three-outcome
      `ResolveTypeExprOutcome::{Done(&'a KType), Park(Vec<NodeId>),
      Unbound(String)}`. Cache miss runs the elaborator against
      `self`, then checks a **finalize gate** before writing:
      every user-type referenced by the result must be fully finalized
      (its name absent from the owning scope's `bindings.pending_types`)
      or the outcome becomes `Park(producers)` and the entry is *not*
      written. The walk is top-level only — SCC closure is atomic
      across members, so a finalized `Foo` guarantees every user-type
      embedded in `Foo`'s payload is also finalized. The memo is
      monotonic: once `(te → kt)` is written, neither key nor value
      changes for the scope's lifetime (Koan data is immutable, and the
      finalize gate prevents caching mid-SCC opaque identities). No
      invalidation, no staleness window. Cache size is bounded by the
      scope's source-form TypeExpr corpus, which is syntactically
      bounded.

  Consumers that need the scope-resolved identity —
  [`type_identity_for`](../src/runtime/machine/core/kfunction/invoke.rs)
  at the dispatch boundary's per-call parameter dual-write,
  [`val_decl::body`](../src/runtime/builtins/val_decl.rs)'s structural
  carrier path and its post-Combine finish, and
  [`fn_def::body`](../src/runtime/builtins/fn_def.rs)'s return-type
  elaboration — go through `Scope::resolve_type_expr`. NEWTYPE's bare-leaf
  user-bound repr path keeps the simpler `Scope::resolve_type` lookup
  (it's intentionally non-park-aware: an unresolvable repr is a hard
  error, not a forward reference). The dispatch boundary's `type_identity_for`
  surfaces a `Park` outcome as the structured
  `KError::TypeIdentityPendingAtDispatch { param, surface, pending_on }`
  rather than silently skipping the dual-write — replaces the prior
  silent-on-`Park` fallback so a workload that triggers it is debuggable
  from the error alone.

  Every `KType` flowing through dispatch is fully elaborated — there is no
  surface-name carrier variant inside `KType` itself.
- Type identity stage 3 — per-declaration `KType::UserType` carrier and
  dual-write. [`enum UserTypeKind { Struct, Tagged, Module, Newtype { repr } }`](../src/runtime/machine/model/types/ktype.rs)
  with a `surface_keyword()` accessor (the `Newtype` variant lands with
  stage 4 below; its `repr` is variant-internal and a manual
  `UserTypeKind::PartialEq` ignores it so wildcard / identity comparisons
  key on kind and `(scope_id, name)` only),
  [`KType::UserType { kind, scope_id, name }`](../src/runtime/machine/model/types/ktype.rs)
  (per-declaration identity tag), and
  [`KType::AnyUserType { kind }`](../src/runtime/machine/model/types/ktype.rs)
  (wildcard kind tag) are the carriers; the old `KType::Struct` /
  `KType::Tagged` / `KType::Module` / `KType::ModuleType` singletons are
  gone. The surface names `"Struct"` / `"Tagged"` / `"Module"` lower to
  `AnyUserType { kind }` in
  [`KType::from_name`](../src/runtime/machine/model/types/ktype_resolution.rs), and
  [`scope.register_type`](../src/runtime/builtins.rs) agrees so the
  type-resolver and the builtin registry produce the same wildcard
  carrier. Predicate arms
  ([`ktype_predicates.rs`](../src/runtime/machine/model/types/ktype_predicates.rs))
  place `UserType { kind: K, .. }` strictly below `AnyUserType { kind: K }`
  strictly below `Any` in `is_more_specific_than`, and `AnyUserType {
  kind }` matches any `KObject::Struct` / `Tagged` / `KModule` of the
  matching kind. Value carriers —
  [`KObject::Struct`](../src/runtime/machine/model/values/kobject.rs),
  [`KObject::Tagged`](../src/runtime/machine/model/values/kobject.rs),
  [`KObject::StructType`](../src/runtime/machine/model/values/kobject.rs),
  [`KObject::TaggedUnionType`](../src/runtime/machine/model/values/kobject.rs),
  and `KObject::KModule` — carry `(scope_id, name)` identity fields
  populated at finalize time via the `scope as *const _ as usize` scheme
  `Module::scope_id()` uses; `ktype()` on a `KObject::Struct` / `Tagged` /
  `KModule` reconstructs `KType::UserType { kind, scope_id, name }`,
  while schema carriers (`StructType` / `TaggedUnionType`) keep reporting
  `KType::Type` (they are values *of the meta-type*, not user-typed
  values). STRUCT / UNION-named / MODULE / SIG finalize each route
  through the
  [`Scope::register_nominal`](../src/runtime/machine/core/scope.rs)
  shim, which transactionally writes `bindings.types[name] = &KType` and
  `bindings.data[name] = &KObject` together so the single-home invariant
  (Type-classed name lookups go through `Scope::resolve_type` only)
  holds — `body_type_expr`'s value-side fall-through is deleted, and the
  resolver's `KSignature` / `StructType` / `TaggedUnionType` value-side
  fallback is gone. `LET <Type-class> = <module/sig/struct-value>` (e.g.
  `LET IntOrdA = (IntOrd :| OrderedSig)`) also dual-writes, preserving
  the *original* carrier's identity rather than minting a fresh
  `scope_id` for the alias name — aliasing is type-equivalent, so a slot
  typed by the alias dispatches to the same overload as a slot typed by
  the original. SIG declarations write `KType::SignatureBound { sig_id,
  sig_path }` on the type side. Anonymous `UNION (...)` is not a valid
  surface — every tagged value carries a real per-declaration identity.
  Mutually recursive STRUCT / named-UNION pairs resolve through the
  [`Bindings.pending_types`](../src/runtime/machine/core/bindings.rs)
  registry: STRUCT / named-UNION `body()` installs a `PendingTypeEntry
  { kind, scope_id, schema_expr, edges }` before launching its
  elaborator; the elaborator's `Resolution::Placeholder` arm records
  edges and runs DFS from `current_decl_name`; a closed cycle invokes
  [`close_type_cycle`](../src/runtime/machine/model/types/resolver.rs), which
  synchronously installs every member's identity into `bindings.types`
  via the panic-on-conflict
  [`Scope::cycle_close_install_identity`](../src/runtime/machine/core/scope.rs)
  shim. Each member's eventual `finalize_struct` / `finalize_union` (or
  Combine-finish for parked members) then routes through
  `try_register_nominal`'s cycle-close-idempotent arm — types is
  pre-populated with a matching identity, so only the carrier writes to
  `data`. Defense-in-depth: every nominal finalize site also short-
  circuits to the existing carrier when both `types[name]` and
  `data[name]` are populated at entry. MODULE does not participate in
  `pending_types` (its body parks on the outer scheduler's sibling
  dispatch deps, not on type-name resolution); the idempotent guard
  still lives in MODULE finalize for symmetry.
- Type identity stage 4 — `NEWTYPE` keyword and `KObject::Wrapped` carrier.
  `NEWTYPE Distance = Number` declares a fresh nominal identity over a
  transparent representation: declaration mints a per-declaration
  [`KType::UserType { kind: UserTypeKind::Newtype { repr: Box<KType> }, scope_id, name }`](../src/runtime/machine/model/types/ktype.rs)
  and writes only `bindings.types` — unlike STRUCT / UNION / MODULE, NEWTYPE
  has no value-side schema carrier (no payload to bind at the declaration
  site). Construction (`Distance(3.0)`, `Bar(Foo(3.0))`) flows through
  `type_call`'s `Newtype` arm into
  [`newtype_def::newtype_construct`](../src/runtime/builtins/newtype_def.rs),
  which schedules the value sub-expression via `add_dispatch` and waits on
  it via a `Combine` whose finish closure type-checks against `repr` and
  produces a
  [`KObject::Wrapped { inner: NonWrappedRef<'a>, type_id: &'a KType }`](../src/runtime/machine/model/values/kobject.rs)
  carrier. Newtype-over-newtype collapse is encoded in the field type:
  [`NonWrappedRef`](../src/runtime/machine/model/values/kobject.rs) is a
  copy-newtype around `&'a KObject<'a>` whose sole constructor `peel` collapses
  any `Wrapped` layer at construction time. `Bar(some_foo)` runs through
  `NonWrappedRef::peel(some_foo)` and rewraps with `Bar`'s `type_id` — at most
  one layer of wrapping at any point, and the invariant is a `cargo check`
  guarantee rather than a caller-discipline contract. The construction path is driven from
  `type_call::body` (which now resolves the verb through `scope.resolve_type`
  first and branches on the resolved `kind`) rather than a second registered
  builtin: a sibling primitive would share `type_call`'s `[TypeExprRef, …]`
  signature bucket and re-dispatch infinitely. ATTR over a `KObject::Wrapped`
  falls through to `inner` via [`access_field`'s `Wrapped`
  arm](../src/runtime/builtins/attr.rs): a new ATTR overload typed
  `AnyUserType { kind: Newtype { repr: Box::new(Any) } }` reuses
  `body_struct` because the lhs-shape dispatch lives inside `access_field`;
  the recursion descends exactly one level by the collapse invariant. The
  ATTR overload's slot is disjoint from the Struct / Module slots (the
  manual `UserTypeKind::PartialEq` discriminates by kind), so dispatch picks
  without a specificity tiebreaker. Missing-field diagnostics name the inner
  struct (`b: Boxed = Point; b.z` reports `struct Point has no field z`) —
  the fall-through is transparent at the diagnostic level too. Stage-4
  routing inside `ktype_predicates` reuses the same `UserType` /
  `AnyUserType { kind: K }` / `Any` specificity stratification stage 3
  established, so `Newtype` ranks alongside `Struct` / `Tagged` / `Module`
  with no per-kind branching at the dispatcher. The wildcard surface name
  `Newtype` is intentionally *not* registered in
  [`KType::from_name`](../src/runtime/machine/model/types/ktype_resolution.rs) —
  it's reserved as the writable form once a builtin signature surfaces the
  need; today it appears only synthesized inside ATTR's `AnyUserType { kind:
  Newtype { repr: Any } }` slot.
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
