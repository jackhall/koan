# Type elaboration

Type elaboration runs in the same scheduler that runs value evaluation.
A type-binding site (`LET Ty = ...`, `STRUCT Ty = ...`, `UNION Ty = ...`)
registers a placeholder in the
[`Bindings`](../../src/runtime/machine/core/scope.rs) façade on `Scope` — the
same `placeholders` table value bindings use, sitting alongside `data` and
`functions` — and dispatches its body as scheduler work.
Lookups of type names from outside the body park on the producer's NodeId
via `notify_list` / `pending_deps`, the same path value-name forward
references take ([execution-model.md § Dispatch-time name placeholders](../execution-model.md#dispatch-time-name-placeholders)).
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
[`access_module_member`](../../src/runtime/builtins/attr.rs) tries the
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

## Binding home and the dual-map

Type bindings live in a separate map from value bindings. The
[`Bindings`](../../src/runtime/machine/core/bindings.rs) façade owns four maps:
`data` for values, `functions` for registered overloads, `placeholders` for
in-flight dispatch tasks, and `types` for type-name → `&KType` arena pointers.
Token-class-driven lookup at the resolver decides which map to consult —
Type-class tokens consult `types`, identifier tokens consult `data`. Builtin
type names *and* `LET Ty = Number`-style aliases live in `bindings.types` as
arena-allocated `&KType`
([`RuntimeArena::alloc_ktype`](../../src/runtime/machine/core/arena.rs)),
reachable through
[`Scope::resolve_type`](../../src/runtime/machine/core/scope.rs) on the same
pointer as the builtin.

[LET routing in `let_binding`](../../src/runtime/builtins/let_binding.rs) detects
Type-class LHS and dispatches through `register_type` for `TypeExprRef`-LHS
RHSes (type-valued aliases). A bind-time
`KErrorKind::TypeClassBindingExpectsType` diagnostic rejects
`LET <Type-class> = <non-type>` at the binder using a primitive/container
blocklist (`Number | Str | Bool | Null | List(_) | Dict(_, _)`) so type-language
carriers (`KModule`, `KSignature`, `StructType`, `TaggedUnionType`), whose
runtime `KType` is `Module` / `Signature` / `Type` rather than `TypeExprRef`,
continue to bind through the existing `bind_value` path.

The value-side ATTR walker and ascription's abstract-type member sweep both
walk `bindings.types` and `bindings.data` via the `abstract_type_names_of`
helper, so SIG `Type` declarations resolve uniformly whether the signature
body's LET wrote to `types` (Type-class LHS, `KTypeValue` RHS) or to `data`
(other type-language carriers).

## Bare-leaf type-name carrier

Bare-leaf type names that aren't in
[`KType::from_name`](../../src/runtime/machine/model/types/ktype.rs)'s builtin
table (`Point`, `IntOrd`, `MyList`) are lowered by
[`ExpressionPart::resolve_for`](../../src/runtime/machine/model/ast.rs) into
`KObject::TypeNameRef(TypeExpr)` rather than a placeholder `KType` variant.
The carrier preserves the parser-side `TypeExpr` for diagnostics and for
consumers that want the user's surface identifier verbatim; it carries no
internal cache. Both `TypeNameRef` and the fully-resolved `KTypeValue` report
`ktype() = KType::TypeExprRef`, so the slot's dispatch position is identical —
whether the surface form already lowered to a concrete `KType` at bind time
or is still in parser-form is an internal detail.

Four downstream consumers each carry a `TypeNameRef` arm beside the existing
`KTypeValue` arm:

- the shared
  [`extract_bare_type_name`](../../src/runtime/machine/core/kfunction/argument_bundle.rs)
  helper (used by STRUCT/UNION declaration sites and `type_call`'s verb slot);
- [ATTR's `body_type_lhs` and `read_field_name`](../../src/runtime/builtins/attr.rs);
- [`let_binding`'s name slot](../../src/runtime/builtins/let_binding.rs), which
  runs the same primitive/container blocklist as the `KTypeValue` arm and
  routes to `register_type` for type-valued RHSes;
- [`value_lookup::body_type_expr`](../../src/runtime/builtins/value_lookup.rs),
  which resolves through `bindings.types` and, on a nominal `UserType` /
  `SignatureBound` hit, recovers the paired value-side carrier from
  `bindings.data`.

FN's deferred return-type elaboration peeks the slot to pick between
[`extract_ktype`](../../src/runtime/machine/core/kfunction/argument_bundle.rs)
(resolved carrier) and the sibling
[`extract_type_name_ref`](../../src/runtime/machine/core/kfunction/argument_bundle.rs)
(deferred carrier consuming the parser-preserved `TypeExpr`), then drives the
existing park-on-placeholder machinery from there. The sole
`KObject::KTypeValue` synthesis site for dispatch transport lives in
[`value_lookup::body_type_expr`](../../src/runtime/builtins/value_lookup.rs),
which mints `KObject::KTypeValue(kt.clone())` on a `resolve_type` hit. On a
`resolve_type` miss, the bare-leaf arm of `elaborate_type_expr` falls through
to `Scope::resolve` for compatibility with the small set of callers that still
consult the value side; the `body_type_expr` reader, by contrast, is
types-only.

Every `KType` flowing through dispatch is fully elaborated — there is no
surface-name carrier variant inside `KType` itself.

## Type-expression resolution memo

Two complementary caches amortize the elaboration cost rather than one cell
on the carrier:

- **Layer 1 — surface-form, scope-independent.** A `OnceCell<KType>` lives on
  [`TypeExpr`](../../src/runtime/machine/model/ast.rs) itself
  (`TypeExpr.builtin_cache`, excluded from `PartialEq` / `Hash`).
  `ExpressionPart::resolve_for` reads the cell first; on miss it runs
  [`KType::from_type_expr`](../../src/runtime/machine/model/types/ktype_resolution.rs)
  and writes the result back when the surface form resolves against the
  builtin table. `from_type_expr` failures (user-bound leaves) are
  intentionally not cached here — those flow through Layer 2. Subsequent
  `KFunction::bind` passes against the same `TypeExpr` pay one
  `OnceCell::get`. No scope keying needed: the cached `KType` depends solely
  on the surface form.

- **Layer 2 — scope-bound resolution memo.** A
  `RefCell<HashMap<TypeExpr, &'a KType>>` lives on
  [`Bindings`](../../src/runtime/machine/core/bindings.rs) (`type_expr_memo`).
  Reached through
  [`Scope::resolve_type_expr`](../../src/runtime/machine/core/scope.rs), which
  returns the three-outcome
  `ResolveTypeExprOutcome::{Done(&'a KType), Park(Vec<NodeId>),
  Unbound(String)}`. Cache miss runs the elaborator against `self`, then
  checks a **finalize gate** before writing: every user-type referenced by the
  result must be fully finalized (its name absent from the owning scope's
  `bindings.pending_types`) or the outcome becomes `Park(producers)` and the
  entry is *not* written. The walk is top-level only — SCC closure is atomic
  across members, so a finalized `Foo` guarantees every user-type embedded in
  `Foo`'s payload is also finalized. The memo is monotonic: once `(te → kt)`
  is written, neither key nor value changes for the scope's lifetime (Koan
  data is immutable, and the finalize gate prevents caching mid-SCC opaque
  identities). No invalidation, no staleness window. Cache size is bounded
  by the scope's source-form TypeExpr corpus, which is syntactically bounded.

Consumers that need the scope-resolved identity —
[`type_identity_for`](../../src/runtime/machine/core/kfunction/invoke.rs)
at the dispatch boundary's per-call parameter dual-write,
[`val_decl::body`](../../src/runtime/builtins/val_decl.rs)'s structural
carrier path and its post-Combine finish, and
[`fn_def::body`](../../src/runtime/builtins/fn_def.rs)'s return-type
elaboration — go through `Scope::resolve_type_expr`. NEWTYPE's bare-leaf
user-bound repr path keeps the simpler `Scope::resolve_type` lookup (it's
intentionally non-park-aware: an unresolvable repr is a hard error, not a
forward reference). The dispatch boundary's `type_identity_for` surfaces a
`Park` outcome as the structured
`KError::TypeIdentityPendingAtDispatch { param, surface, pending_on }` rather
than silently skipping the dual-write, so a workload that triggers it is
debuggable from the error alone.
