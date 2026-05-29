# Type elaboration

Type elaboration runs in the same scheduler that runs value evaluation.
A type-binding site (`LET Ty = ...`, `STRUCT Ty = ...`, `UNION Ty = ...`)
registers a placeholder in the
[`Bindings`](../../src/machine/core/scope.rs) façade on `Scope` — the
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
[`access_module_member`](../../src/builtins/attr.rs) tries the
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

## Layers

The pipeline from parser [`TypeExpr`](../../src/machine/model/ast.rs) to
fully-elaborated `&'a KType` runs through five layers, each with a
distinct source-file home. Other typing docs that touch a single layer
cross-link this section rather than restating its slice.

- **Layer 1 — surface-form builtin cache** in
  [`ast.rs`](../../src/machine/model/ast.rs). A `OnceCell<KType>` on
  `TypeExpr` itself memoizes scope-independent builtin lowering.
  `ExpressionPart::resolve_for` reads the cell first; misses run
  [`KType::from_type_expr`](../../src/machine/model/types/ktype_resolution.rs)
  and write back when the surface form resolves against the builtin
  table. Arity is enforced here, before binder install. See
  [Strict admission rules](#strict-admission-rules) below for the cache
  mechanics.
- **Layer 2 — scope-bound elaboration memo** in
  [`bindings.rs`](../../src/machine/core/bindings.rs). A
  `RefCell<HashMap<TypeExpr, &'a KType>>` on `Bindings` (`type_expr_memo`)
  caches resolved `TypeExpr → &'a KType` per scope, gated by a finalize
  check on every embedded user-type. Reached through
  [`Scope::resolve_type_expr`](../../src/machine/core/scope.rs), which
  returns the three-outcome
  `ResolveTypeExprOutcome::{Done, Park, Unbound}`. See
  [Strict admission rules](#strict-admission-rules) for the gate and
  the monotonicity argument.
- **Layer 3 — the elaborator** in
  [`resolver.rs`](../../src/machine/model/types/resolver.rs). Recursive
  walk over `TypeExpr` that threads the set of binders currently being
  elaborated for self-reference recognition (described above), parks on
  not-yet-finalized leaves via `ElabResult::Park(producers)`, and
  produces `&'a KType`. FN-signature, STRUCT/UNION field-type, and
  FUNCTOR per-call return-type Combines all reduce to this walk; see
  [execution-model.md § Dispatch-time name placeholders](../execution-model.md#dispatch-time-name-placeholders)
  for the parking integration.
- **Layer 4 — bare-leaf dispatch ingress** in
  [`resolve_type_expr.rs`](../../src/machine/execute/dispatch/resolve_type_expr.rs).
  [`coerce_type_token_value`](../../src/machine/execute/dispatch/resolve_type_expr.rs)
  is the shared coercion seam from a bare-`Type` token to a dispatch-time
  carrier, called from the dispatcher's `BareTypeLeaf` fast lane and the
  keyworded splice walk's eager name-resolve pass. Resolves through
  `bindings.types` and synthesizes `KObject::KTypeValue(kt.clone())` on a
  non-nominal hit; nominal hits recover the paired value-side carrier
  from `bindings.data`. See
  [Bare-leaf type-name carrier](#bare-leaf-type-name-carrier) below for
  the downstream consumers.
- **Layer 5 — surface-form-survives-bind carrier** in
  [`kobject.rs`](../../src/machine/model/values/kobject.rs).
  `KObject::TypeNameRef(TypeExpr)` preserves the parser-side `TypeExpr`
  verbatim for bare-leaf type names not in the builtin table — so
  diagnostics quote the user's identifier exactly as written rather than
  the elaborated canonical form. See
  [Bare-leaf type-name carrier](#bare-leaf-type-name-carrier) for the
  consumers that carry a `TypeNameRef` arm beside the `KTypeValue` arm.

## Binding-map partition

Type bindings live in a separate map from value bindings — the type-side
slice of the [lookup → admit protocol](lookup-protocol.md)'s Layer 2.
The [`Bindings`](../../src/machine/core/bindings.rs) façade owns four
maps: `data` for values, `functions` for registered overloads,
`placeholders` for in-flight dispatch tasks, and `types` for type-name →
`&KType` arena pointers. Token-class-driven lookup at the resolver
decides which map to consult — Type-class tokens consult `types`,
identifier tokens consult `data`. Builtin type names *and* `LET Ty =
Number`-style aliases live in `bindings.types` as arena-allocated
`&KType` ([`RuntimeArena::alloc_ktype`](../../src/machine/core/arena.rs)),
reachable through
[`Scope::resolve_type`](../../src/machine/core/scope.rs) on the same
pointer as the builtin.

[LET routing in `let_binding`](../../src/builtins/let_binding.rs) detects
Type-class LHS and dispatches through `register_type` for `TypeExprRef`-LHS
RHSes (type-valued aliases). A bind-time
`KErrorKind::TypeClassBindingExpectsType` diagnostic gates the RHS via an
**allowlist**: a Type-class LET admits a value only if it carries
type-language identity in one of three shapes — `KObject::KTypeValue(_)`
(pure-type carriers including `KType::Module` / `KType::Signature`),
`derive_nominal_identity → Some(_)` (`StructType` / `TaggedUnionType`,
which redundantly subsumes the `Module` / `Signature` `KTypeValue` arm),
or `KObject::KFunction(f, _)` with `f.is_functor` set (the `FUNCTOR`
binder's output). Plain `KFunction` rejects, closing the
`LET Plain = (FN …)`-binds-a-plain-function-under-a-Type-class-name hole
that a pure value-shape gate cannot discriminate; the `is_functor` flag
is the discrimination signal. Module and signature LET aliases route through
`register_nominal` to install the identity carrier into `bindings.types`
alongside the value-side carrier in `bindings.data` (modules preserve their
`KType::Module` carrier verbatim; signatures lower to the
`KType::SatisfiesSignature` constraint form so a slot typed by the alias
dispatches identically to the original); pure-type `KTypeValue(kt)`
carriers (Number, etc.) take `register_type` directly; `is_functor`
KFunctions and `Struct` / `Tagged` carriers fall through to `bind_value`.

The partition is one-way: a value-classified LET (lowercase-leading binder
name) rejects a `KType::Module` or `KType::Signature` RHS at the LET site
with a `ShapeError` redirecting the user to a Type-classified name. Combined
with the Type-class LET allowlist above, this makes `bindings.types` the
single home for module and signature values — a module value never rides a
value-classified alias, so the value-side lookup and type-class lookup
paths never both find a module under the same name. The
[token-class rule](tokens.md) defines the Type-class shape
(uppercase-leading plus at least one lowercase letter); the partition guard
lives in [`let_binding`'s `body`](../../src/builtins/let_binding.rs).

The value-side ATTR walker and ascription's abstract-type member sweep both
walk `bindings.types` and `bindings.data` via the `abstract_type_names_of`
helper, so SIG `Type` declarations resolve uniformly whether the signature
body's LET wrote to `types` (Type-class LHS, `KTypeValue` RHS) or to `data`
(other type-language carriers).

## Bare-leaf type-name carrier

Bare-leaf type names that aren't in
[`KType::from_name`](../../src/machine/model/types/ktype.rs)'s builtin
table (`Point`, `IntOrd`, `MyList`) are lowered by
[`ExpressionPart::resolve_for`](../../src/machine/model/ast.rs) into
`KObject::TypeNameRef(TypeExpr)` rather than a placeholder `KType` variant.
The carrier preserves the parser-side `TypeExpr` for diagnostics and for
consumers that want the user's surface identifier verbatim; it carries no
internal cache. Both `TypeNameRef` and the fully-resolved `KTypeValue` report
`ktype() = KType::TypeExprRef`, so the slot's dispatch position is identical —
whether the surface form already lowered to a concrete `KType` at bind time
or is still in parser-form is an internal detail.

Three downstream consumers each carry a `TypeNameRef` arm beside the existing
`KTypeValue` arm:

- the shared
  [`extract_bare_type_name`](../../src/machine/core/kfunction/argument_bundle.rs)
  helper (used by STRUCT/UNION declaration sites and the dispatcher's
  `ConstructorCall` fast lane);
- [ATTR's `body_type_lhs` and `read_field_name`](../../src/builtins/attr.rs);
- [`let_binding`'s name slot](../../src/builtins/let_binding.rs), which
  runs the same primitive/container blocklist as the `KTypeValue` arm and
  routes to `register_type` for type-valued RHSes.

The single-part `<v:TypeExpr>` lookup that those consumers' siblings used to
need is now folded into the dispatcher's `BareTypeLeaf` fast lane
([`dispatch/single_poll.rs`](../../src/machine/execute/dispatch/single_poll.rs)),
which calls
[`coerce_type_token_value`](../../src/machine/execute/dispatch/resolve_type_expr.rs)
directly — the shared coercion seam also called from the keyworded splice
walk's eager name-resolve pass
([`dispatch.rs`](../../src/machine/execute/dispatch.rs)).
The helper resolves through `bindings.types` and, on a nominal
`UserType` / `SatisfiesSignature` / `Module` / `Signature` hit, recovers
the paired value-side carrier from `bindings.data`.

FN's deferred return-type elaboration peeks the slot to pick between
[`extract_ktype`](../../src/machine/core/kfunction/argument_bundle.rs)
(resolved carrier) and the sibling
[`extract_type_name_ref`](../../src/machine/core/kfunction/argument_bundle.rs)
(deferred carrier consuming the parser-preserved `TypeExpr`), then drives the
existing park-on-placeholder machinery from there. The sole
`KObject::KTypeValue` synthesis site for dispatch transport lives in
[`coerce_type_token_value`](../../src/machine/execute/dispatch/resolve_type_expr.rs),
which mints `KObject::KTypeValue(kt.clone())` on a non-nominal `resolve_type`
hit. On a `resolve_type` miss, the bare-leaf arm of `elaborate_type_expr`
falls through to `Scope::resolve` for compatibility with the small set of
callers that still consult the value side; the `coerce_type_token_value`
reader, by contrast, is types-only.

Every `KType` flowing through dispatch is fully elaborated — there is no
surface-name carrier variant inside `KType` itself.

## Strict admission rules

[`signature_admits_strict`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
admits a candidate signature against an expression by walking slot/part
pairs and consulting the per-`run_dispatch` `bare_outcomes` cache. The
admission rule per cache entry on a bare-name part:

| Cache entry              | Admission rule                                                                   |
|--------------------------|----------------------------------------------------------------------------------|
| `Resolved(obj)`          | Admit iff [`KType::accepts_part`](../../src/machine/model/types/ktype_predicates.rs) accepts `Future(obj)`. A wrong carried type strict-rejects rather than tentative-admitting into a bind-time `TypeMismatch`. |
| `Parked` / `Unbound`     | Admit via shape-only `arg.matches(part)`. The post-pick splice/park walk is the only place that produces precise per-slot `ParkOnProducers` / `UnboundName` diagnostics, so admission must not reject and lose them. |
| `ProducerErrored`        | Defensive reject. The upfront sweep short-circuits this case before resolution; reaching admission means a producer error slipped past, so refuse. |
| `Cycle`                  | Unreachable. The cache is built with `consumer = None`, so cycle detection never fires during admission. |
| `None` (non-bare part)   | Fall back to shape-only `arg.matches(part)`.                                     |

**Binder declaration slots bypass the cache.** A slot typed `KType::Identifier`
or `KType::TypeExprRef` owns the name (`x` in `LET x = …`, `Ty` in
`STRUCT Ty = …`), so admission must be shape-only regardless of whether
the name happens to be bound elsewhere. A `SigiledTypeExpr` part still
admits speculatively in such a slot — it sub-dispatches to a type-side
carrier downstream. The same shape-only-on-binder-slot rule covers
`KExpression` slots: the slot owns its body, not a name lookup.

Two complementary caches amortize the elaboration cost rather than one cell
on the carrier:

- **Layer 1 — surface-form, scope-independent.** A `OnceCell<KType>` lives on
  [`TypeExpr`](../../src/machine/model/ast.rs) itself
  (`TypeExpr.builtin_cache`, excluded from `PartialEq` / `Hash`).
  `ExpressionPart::resolve_for` reads the cell first; on miss it runs
  [`KType::from_type_expr`](../../src/machine/model/types/ktype_resolution.rs)
  and writes the result back when the surface form resolves against the
  builtin table. `from_type_expr` failures (user-bound leaves) are
  intentionally not cached here — those flow through Layer 2. Subsequent
  `KFunction::bind` passes against the same `TypeExpr` pay one
  `OnceCell::get`. No scope keying needed: the cached `KType` depends solely
  on the surface form.

- **Layer 2 — scope-bound resolution memo.** A
  `RefCell<HashMap<TypeExpr, &'a KType>>` lives on
  [`Bindings`](../../src/machine/core/bindings.rs) (`type_expr_memo`).
  Reached through
  [`Scope::resolve_type_expr`](../../src/machine/core/scope.rs), which
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
[`type_identity_for`](../../src/machine/core/kfunction/invoke.rs)
at the dispatch boundary's per-call type-side bind,
[`val_decl::body`](../../src/builtins/val_decl.rs)'s structural
carrier path and its post-Combine finish, and
[`fn_def::body`](../../src/builtins/fn_def.rs)'s return-type
elaboration — go through `Scope::resolve_type_expr`. NEWTYPE's bare-leaf
user-bound repr path keeps the simpler `Scope::resolve_type` lookup (it's
intentionally non-park-aware: an unresolvable repr is a hard error, not a
forward reference). The dispatch boundary's `type_identity_for` surfaces a
`Park` outcome as the structured
`KError::TypeIdentityPendingAtDispatch { param, surface, pending_on }` rather
than silently skipping the per-call bind, so a workload that triggers it is
debuggable from the error alone.
