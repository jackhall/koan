# Type elaboration

Type elaboration runs in the same scheduler that runs value evaluation.
A type-binding site (`LET Ty = ...`, `STRUCT Ty = ...`, `UNION Ty = ...`)
registers a placeholder in the
[`Bindings`](../../src/machine/core/scope.rs) façade on `Scope` — the
same `placeholders` table value bindings use, sitting alongside `data` and
`functions` — and dispatches its body as scheduler work.

**Type names obey strict source order.** The elaborator carries an
`Option<Rc<LexicalFrame>>` chain and resolves a bare leaf through
[`resolve_type_with_chain`](../../src/machine/core/scope.rs) /
[`resolve_with_chain`](../../src/machine/core/scope.rs), gating each candidate
against the consumer's lexical position by `idx < cutoff` — exactly the value
language's rule. A type binding declared lexically later than its consumer is
invisible, so a forward type reference is a *position error*, not a silent
success or a park.
[`Scope::resolve_type_expr`](../../src/machine/core/scope.rs) takes the chain and
the `type_expr_memo` re-keys by `(TypeName, cutoff)`, so a forward and a backward
consumer at the same scope never share a cached verdict. The `Resolution::Placeholder`
arm parks only on an *earlier still-finalizing* type (a binder visible at the
consumer's position whose body has not finished); `pending_types` survives only
to mark which binders are in flight. This composes with value-name forward
references uniformly — both are lexically gated
([execution-model.md § Dispatch-time name placeholders](../execution-model.md#dispatch-time-name-placeholders)).

**Self-recursion threads the declaring name.** A binder threads its own name into
its body, so a self-reference (`STRUCT Tree = (left :Tree)`) lowers to a transient
[`KType::RecursiveRef(name)`](../../src/machine/model/types/ktype.rs) rather than
forward-referencing a not-yet-visible binding. At the member's finalize,
`seal_recursive_refs` seals every `RecursiveRef` whose name is a set member to a
`KType::SetLocal(index)` against the singleton set the binder seals. The transient
never survives into a sealed type and never reaches the predicates.

**`RECURSIVE TYPES` is the mutual-recursion mechanism.** A cycle of two or more
nominal types has no valid source order, so it is co-declared in a
`RECURSIVE TYPES Name = (...)` block (see
[user-types.md § `RECURSIVE TYPES`](user-types.md#recursive-types--the-mutual-recursion-construct)).
The block threads every member name and scopes the threaded group within strict
lexical order: a cross-reference lowers to a transient `RecursiveRef` and seals to
a `SetLocal` index into one shared `RecursiveSet`. Exiting the block guarantees
every forward reference resolved — a member that never seals is an error at the
block boundary, so no unresolved forward reference escapes.

**One canonical runtime type representation.** A type flows raw as a `&KType` in the
value channel's `Type` arm ([`Carried::Type`](../../src/machine/model/values/carried.rs)),
and a type binding finalizes to the arena `&KType` in `bindings.types`. Consumers read the
elaborated type directly; there is no surface/elaborated split, no per-lookup
re-elaboration, no parallel surface-name representation flowing through
dispatch. A recursive group's identity is its `RecursiveSet` pointer, so
cycle-aware traversals (equality, printing, hashing) key on
`(Rc::as_ptr(set), index)` without descending the cyclic schema. Trivially
cyclic aliases (`LET Ty = Ty`) surface as a structured error rather than a
stack overflow.

**Module-qualified type names.** A `TypeName` like `Mo.Ty` or chained
`Outer.Inner.T` resolves through the value-side ATTR walker:
[`access_module_member`](../../src/builtins/attr.rs) tries the
module's `type_members` table (opaque-ascription type bindings), then
the child scope's `data` (so chained `Outer.Inner.X` reads the inner
*module value* and the chain stays drillable), then the child scope's
type-side `bindings.types` via `Scope::resolve_type` — surfacing the type in the value
channel's `Type` arm so type-position consumers (e.g. a LET-RHS routing
through Combine) see a first-class `KType` value. The resolved type is
the leaf's existing per-declaration `KType::SetRef { set, index }`; no new
variant, no path field.

**Forward type aliases are position errors.** A top-level `LET Ty = Un; LET Un =
Number` rejects: the Type-classed `Un` token on the first LET's RHS resolves under
the same `idx < cutoff` chain gate as a value reference, and `Un` is not yet
visible at `Ty`'s position. A source-order alias (`LET Un = Number; LET Ty = Un`)
binds normally and writes through `Scope::register_type` to land in
`bindings.types`. Mutual recursion that genuinely needs a cycle uses a
`RECURSIVE TYPES` block.

## Every definition-time site is gated to its binder's position

A definition-time type resolution is gated to the lexical position of the binder
that owns it: STRUCT / UNION / NEWTYPE field types, FN and FUNCTOR parameter
slots, and FN / MATCH / TRY return types all resolve against the chain at their
binder's cutoff (`classify_return_type` / `resolve_arm_return_contract` thread the
chain for the return-type sites). A field or parameter naming a type declared
later is a position error, the same rule the value language enforces.

A *deferred* return type — one that references a parameter, like a functor's
`-> Er` — is the one definition-time site with no forward-reference question to
gate. It resolves at **call time** against the per-call scope, where the
parameters bind at index 0 (visible) and every outer type is already finalized.
There is no later-than-the-consumer binding to reject, so the deferred path
resolves directly.

## Layers

The parser's bare type-leaf carrier is
[`TypeName`](../../src/machine/model/ast.rs), a thin newtype over the source
name (`Deref`s to `str`, derives eq/hash by string). The pipeline from a
`TypeName` to a fully-elaborated `&'a KType` runs through five layers, each with
a distinct source-file home. Other typing docs that touch a single layer
cross-link this section rather than restating its slice.

- **Layer 1 — bind-time builtin lowering** in
  [`ast.rs`](../../src/machine/model/ast.rs).
  `ExpressionPart::resolve_for` lowers a bare `Type` token against the
  builtin table via
  [`KType::from_type_expr`](../../src/machine/model/types/ktype_resolution.rs)
  (a match over the ~10-entry builtin map, re-run per call). A hit lowers to a resolved
  `&KType` in the value channel's `Type` arm; a miss — a user-bound leaf — defers to the
  [`KType::Unresolved(TypeName)`](../../src/machine/model/types/ktype.rs) transient, which
  preserves the parser-side name verbatim until the park-capable
  `Scope::resolve_type_expr` consumes it. Runs at `KFunction::bind` time, which has no
  `Scope` in hand, so it is builtin-only and scope-independent.
- **Layer 2 — scope-bound elaboration memo (the sole cache tier)** in
  [`bindings.rs`](../../src/machine/core/bindings.rs). A
  `RefCell<HashMap<(TypeName, Option<usize>), &'a KType>>` on `Bindings`
  (`type_expr_memo`) caches resolved `(TypeName, cutoff) → &'a KType` per scope,
  gated by a finalize check on every embedded user-type. Keying by `cutoff` keeps
  a forward consumer (which sees a name as unresolved) and a backward consumer
  (which sees it bound) from sharing a verdict. Reached through
  [`Scope::resolve_type_expr`](../../src/machine/core/scope.rs), which takes the
  chain and returns the three-outcome
  `ResolveTypeExprOutcome::{Done, Park, Unbound}`. See
  [Strict admission rules](#strict-admission-rules) for the gate and
  the monotonicity argument.
- **Layer 3 — the elaborator** in
  [`resolver.rs`](../../src/machine/model/types/resolver.rs). Resolves a
  *bare-leaf* `TypeName` against the scope into `&'a KType`, carrying the
  consumer's `LexicalFrame` chain so each candidate is gated by `idx < cutoff`: a
  threaded binder name (its own, or a `RECURSIVE TYPES` group sibling) turns into
  a transient `KType::RecursiveRef`, an *earlier still-finalizing* binder parks via
  `ElabResult::Park(producers)`, a later-than-the-consumer binding is a position
  error, and a builtin name falls back to `KType::from_name`. Parameterized shapes
  (`:(LIST OF X)`, `:(MAP K -> V)`) sub-Dispatch through the standalone dispatcher
  rather than recursing here, so the only recursion is the sibling-result reduce.
  FN-signature, STRUCT/UNION field-type, and FUNCTOR per-call return-type Combines
  reduce to this leaf walk; see
  [execution-model.md § Dispatch-time name placeholders](../execution-model.md#dispatch-time-name-placeholders)
  for the parking integration.
- **Layer 4 — bare-leaf dispatch ingress** in
  [`resolve_type_expr.rs`](../../src/machine/execute/dispatch/resolve_type_expr.rs).
  [`resolve_type_leaf_carrier`](../../src/machine/execute/dispatch/resolve_type_expr.rs)
  is the shared seam from a bare-`Type` token to a dispatch-time carrier,
  called from the dispatcher's `BareTypeLeaf` fast lane and the keyworded
  splice walk's eager name-resolve pass. It wraps the same memoized,
  park-capable `Scope::resolve_type_expr` bridge (Layer 2) every compound type
  form uses, returning `TypeLeafCarrier::{Resolved, Park, Unbound}`: `Resolved`
  carries the bridge's cached `&KType` raw in the value channel's `Type` arm, and `Park`
  lets a leaf naming an earlier still-finalizing binder park on its producer and
  re-resolve on wake. See
  [Bare-leaf type-name carrier](#bare-leaf-type-name-carrier) below for
  the downstream consumers.
- **Layer 5 — surface-form-survives-bind transient** in
  [`ktype.rs`](../../src/machine/model/types/ktype.rs).
  [`KType::Unresolved(TypeName)`](../../src/machine/model/types/ktype.rs) preserves the
  parser-side `TypeName` verbatim for bare-leaf type names not in the builtin table — so
  diagnostics quote the user's identifier exactly as written rather than the elaborated
  canonical form. A transient sibling to `RecursiveRef`, it never reaches the dispatch
  predicates: `Scope::resolve_type_expr` consumes and replaces it. See
  [Bare-leaf type-name carrier](#bare-leaf-type-name-carrier) for the consumers.

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

STRUCT / UNION / MODULE / Result / SIG declarations are all single
type-namespace writes: each installs only its identity-bearing `&KType` into
`bindings.types` (see
[type-only nominal install](user-types.md#type-only-nominal-install)), so
`bindings.data` carries **zero** type carriers — the type-language /
value-language partition is total. There is no value-side schema or signature
carrier; construction reads the schema straight off the identity, and a
signature value is synthesized on demand from the type entry. SIG's single
`KType::Signature { sig, pinned_slots }` variant serves both its constraint
role and its value role, so it installs one type-side identity like every other
nominal binder — no binder dual-writes `(bindings.types, bindings.data)`.

[LET routing in `let_binding`](../../src/builtins/let_binding.rs) detects
Type-class LHS and dispatches through `register_type` for `TypeExprRef`-LHS
RHSes (type-valued aliases). A bind-time
`KErrorKind::TypeClassBindingExpectsType` diagnostic gates the RHS via an
**allowlist**, `is_admissible_type_class_rhs`: a Type-class LET admits an RHS
only if it carries type-language identity — any value-channel `Type` arm
(struct / union / module / Result / signature identities all flow raw as `&KType`),
or `KObject::KFunction(f, _)` with `f.is_functor` set (the
`FUNCTOR` binder's output). Plain `KFunction` rejects, closing the
`LET Plain = (FN …)`-binds-a-plain-function-under-a-Type-class-name hole
that a pure value-shape gate cannot discriminate; the `is_functor` flag
is the discrimination signal. The lockstep partner `type_side_identity` maps
each admitted value to the `KType` identity that lands in `bindings.types`: a
`Type`-arm `kt` registers `kt` directly, while a bound functor registers its
`KType::KFunctor { body: Some(f) }` projection so the callable rides the
type-table identity and a later `:(F {…})` / `F {…}` application can invoke it
(see [functors.md § Application and binding](functors.md#application-and-binding)).
The two functions must agree: anything the allowlist admits must produce a
type-side identity here, or a functor would fall through to `bindings.data`.
Every type-language alias — struct / union / module / Result, signature *and*
bound functor — routes through `register_type` (type-only): the schema,
`&Module`, `&Signature`, or callable rides the `KType` identity, so a plain
`types` write preserves dispatch identity without a value-side copy. A
`LET S2 = OrderedSig` signature alias therefore dispatches identically to the
original, with no separate nominal-install path.

The partition is one-way and total against type-language carriers. A
value-classified LET (lowercase-leading binder name) rejects **any** type
RHS at the LET site with a `ShapeError` redirecting the user to a
Type-classified name: every value-channel `Type` arm (struct / union / module /
Result / signature / builtin type, including the `KType::Unresolved` parser-form
transient), plus an `is_functor`-flagged `KFunction` (a functor lives in the type
namespace only). A type therefore binds only under a Type-classified name; construction
names the type directly (`Point {…}`) or through a Type-classified alias
(`LET Pt2 = Point` then `Pt2 {…}`), never a value-classified one. Combined with
the Type-class LET allowlist above, this makes `bindings.types` the single home
for every type identity, so `bindings.data` is unconditionally free of
type-language carriers and the value-side and type-class lookup paths never both
find one under the same name. The [token-class rule](tokens.md) defines the
Type-class shape (uppercase-leading plus at least one lowercase letter); the
partition guard lives in
[`let_binding`'s `body`](../../src/builtins/let_binding.rs).

The value-side ATTR walker and ascription's abstract-type member sweep both
walk `bindings.types` and `bindings.data` via the `abstract_type_names_of`
helper, so SIG `Type` declarations resolve uniformly whether the signature
body's LET wrote to `types` (Type-class LHS, `Type`-arm RHS) or to `data`
(other type-language carriers).

## Bare-leaf type-name carrier

Bare-leaf type names that aren't in
[`KType::from_name`](../../src/machine/model/types/ktype.rs)'s builtin
table (`Point`, `IntOrd`, `MyList`) are lowered by
[`ExpressionPart::resolve_for`](../../src/machine/model/ast.rs) into the
[`KType::Unresolved(TypeName)`](../../src/machine/model/types/ktype.rs) transient — riding
the value channel's `Type` arm like any other type — rather than a resolved `&KType`.
The transient preserves the parser-side `TypeName` for diagnostics and for
consumers that want the user's surface identifier verbatim. Both `Unresolved`
and a fully-resolved type carry in the `Type` arm and report
`ktype() = KType::OfKind(Proper)`, so the slot's dispatch position is identical —
whether the surface form already lowered to a concrete `KType` at bind time
or is still in parser-form is an internal detail.

Downstream consumers branch on the `Type` arm, handling the `Unresolved` transient where
a bare user name can still be pending:

- the shared
  [`require_bare_type_name`](../../src/machine/core/kfunction/action.rs)
  helper (used by the nominal binders that read their name from a `KObject::Record`
  type cell), which renders either an `Unresolved` name or a resolved type;
- [ATTR's `body_type_lhs` and `read_field_name`](../../src/builtins/attr.rs);
- [`let_binding`'s name slot](../../src/builtins/let_binding.rs), which
  runs a primitive/container blocklist over the `Type` arm and
  routes to `register_type` for type-valued RHSes.

The single-part bare-`Type` lookup that those consumers' siblings need is
folded into the dispatcher's `BareTypeLeaf` fast lane
([`dispatch/single_poll.rs`](../../src/machine/execute/dispatch/single_poll.rs)),
which calls
[`resolve_type_leaf_carrier`](../../src/machine/execute/dispatch/resolve_type_expr.rs)
— the shared seam also called from the keyworded splice walk's eager
name-resolve pass
([`dispatch.rs`](../../src/machine/execute/dispatch.rs)).
The seam wraps the memoized, park-capable `Scope::resolve_type_expr` bridge: on
a resolved leaf it surfaces the bridge's cached `&KType` in the value channel's `Type` arm
for every type-only nominal — struct / union / module / Result *and* signature; on an
earlier still-finalizing binder it parks; on a miss it surfaces `Unbound`.

FN's deferred return-type slot is parsed at definition time via
[`extract_return_type_raw`](../../src/builtins/fn_def/return_type.rs), which reads any
`Type`-arm type — a resolved `&KType` or the `KType::Unresolved` transient for a bare leaf —
and branches on `Unresolved` to pick the `TypeExpr` carrier (see
[fn_def/return_type.rs](../../src/builtins/fn_def.rs)). At call time the body executor
[`run_user_fn`](../../src/machine/core/kfunction/exec.rs) elaborates that `TypeExpr` carrier
inline against the per-call child scope (`elaborate_type_expr`), where the param install has
already finalized every parameter-name identity; type-denoting parameters themselves bind via
`register_type` from an already-resolved type argument, so there is no transient identity
elaboration at the bind site. The sole bare-leaf resolution site for dispatch transport
lives in
[`resolve_type_leaf_carrier`](../../src/machine/execute/dispatch/resolve_type_expr.rs),
which surfaces the bridge's resolved `&KType`. Bare leaves resolve through the same
memo and parking discipline as compound type forms — there is no separate
synchronous bare-leaf path.

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
the name happens to be bound elsewhere. A `SigiledTypeExpr` or `RecordType`
part still admits speculatively in such a slot — it sub-dispatches to a
type-side carrier downstream. The same shape-only-on-binder-slot rule covers
`KExpression` slots: the slot owns its body, not a name lookup.

A single cache tier amortizes the elaboration cost. Bind-time builtin lowering
([`ExpressionPart::resolve_for`](../../src/machine/model/ast.rs) →
[`KType::from_type_expr`](../../src/machine/model/types/ktype_resolution.rs))
re-runs the ~10-entry builtin match per call rather than memoizing it — the
match is cheap and a shared table would be added back only if profiling shows
it hot. The scope-bound resolution memo is therefore the only cache:

- A `RefCell<HashMap<TypeName, &'a KType>>` lives on
  [`Bindings`](../../src/machine/core/bindings.rs) (`type_expr_memo`).
  Reached through
  [`Scope::resolve_type_expr`](../../src/machine/core/scope.rs), which
  returns the three-outcome
  `ResolveTypeExprOutcome::{Done(&'a KType), Park(Vec<NodeId>),
  Unbound(String)}`. Cache miss runs the elaborator against `self`, then
  checks a **finalize gate** before writing: every user-type referenced by the
  result must be fully finalized (its name absent from the owning scope's
  `bindings.pending_types`) or the outcome becomes `Park(producers)` and the
  entry is *not* written. The walk is top-level only — a `RECURSIVE TYPES` block
  seals every member together, so a finalized `Foo` guarantees every user-type
  embedded in `Foo`'s payload is also finalized. The memo is monotonic: once
  `((te, cutoff) → kt)` is written, neither key nor value changes for the scope's
  lifetime (Koan data is immutable, and the finalize gate prevents caching a
  member before its group sealed). No invalidation, no staleness window. Cache
  size is bounded by the scope's source-form `TypeName` corpus paired with the
  finite set of consumer cutoffs, which is syntactically bounded.

Consumers that need the scope-resolved identity —
[`val_decl::body`](../../src/builtins/val_decl.rs)'s structural
carrier path and its post-Combine finish, and
[`fn_def::body`](../../src/builtins/fn_def.rs)'s return-type
elaboration — go through `Scope::resolve_type_expr`. NEWTYPE's bare-leaf
user-bound repr path keeps the simpler `Scope::resolve_type` lookup (it's
intentionally non-park-aware: an unresolvable repr is a hard error, not a
forward reference). A type-denoting FN parameter binds its already-resolved type
argument directly via `register_type` in
[`run_user_fn`](../../src/machine/core/kfunction/exec.rs), so the per-call
type-side bind needs no scope re-resolution and cannot park.
