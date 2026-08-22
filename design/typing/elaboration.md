# Type elaboration

Type elaboration runs in the same scheduler that runs value evaluation.
A type-binding site (`LET Ty = ...`, `NEWTYPE Ty = ...`, `UNION Ty = ...`)
registers a claim in the
[`Bindings`](../../src/machine/core/scope.rs) façade on `Scope` — a claim on the
name in the scope's claim store, the type-side use of the very entry a value
binding claims — and dispatches its body as
scheduler work.

**Type names obey strict source order.** The elaborator carries an
`Option<Rc<LexicalFrame>>` chain and resolves a bare leaf through
[`resolve_type_with_chain`](../../src/machine/core/scope.rs) /
[`resolve_with_chain`](../../src/machine/core/scope.rs), gating each candidate
against the consumer's lexical position by `idx < cutoff` — exactly the value
language's rule. A type binding declared lexically later than its consumer is
invisible, so a forward type reference is a *position error*, not a silent
success or a park.
[`Scope::resolve_type_identifier`](../../src/machine/execute/decide/resolve_type_identifier.rs) takes the chain, so a
forward and a backward consumer at the same scope reach different verdicts. The
`NameLookup::Parked` arm parks only on an *earlier still-finalizing* type (a binder visible
at the consumer's position whose body has not finished); the binder's type-side placeholder
is what marks it in flight. This composes with value-name forward
references uniformly — both are lexically gated
([execution/name-placeholders.md § Dispatch-time name placeholders](../execution/name-placeholders.md#dispatch-time-name-placeholders)).

**Self-recursion threads the declaring name.** A binder threads its own name into
its body, so a self-reference (`NEWTYPE Tree = :{left :Tree}`) lowers to a relative
[`TypeNode::Sibling`](../../src/machine/model/types/node.rs) reference against the
binder's group window rather than forward-referencing a not-yet-visible binding. At
seal, the window rewrites every `Sibling` whose name is a member to that member's
absolute handle in its singleton component. The relative reference
never survives into a sealed schema and never reaches the predicates.

**A module body's announcement is the mutual-recursion mechanism.** A cycle of two or
more nominal types has no valid source order, so it is co-declared in a module body,
whose top-level type declarations are announced before any of them runs (see
[user-types.md § Mutual recursion](user-types.md#mutual-recursion--the-module-body-announcement)).
The announcement is the body's ambient declaration window: a cross-reference by the
member's own declarator lowers to a relative `Sibling` and seals to the sibling's
absolute member handle, and a reference by a plain consumer parks until the window
seals and then reads the absolute handle. This is the one cross-order type-name
resolution that survives strict lexical lookup, and it is scoped to the body — a
member that never fills is a typed error at the module's finish, so no unresolved
forward reference escapes.

**One canonical runtime type representation.** A type flows raw as a `KType` handle in the
value channel's `Type` arm ([`Carried::Type`](../../src/machine/model/values/carried.rs)),
and a type binding finalizes to a `KType` in `bindings.types`. Consumers read the
elaborated type directly; there is no surface/elaborated split, no per-lookup
re-elaboration, no parallel surface-name representation flowing through
dispatch. A nominal member's identity is its `(SCC digest, index)` handle, so
cycle-aware traversals (equality, printing, hashing) key on that `Copy` handle
without descending the cyclic schema. Trivially
cyclic aliases (`LET Ty = Ty`) surface as a structured error rather than a
stack overflow.

**Module-qualified type names.** A `TypeName` like `mo.Ty` or chained
`outer.inner.T` resolves through the value-side ATTR walker:
[`access_module_member`](../../src/builtins/attr.rs) tries the
module's `type_members` table (opaque-ascription type bindings), then
the child scope's `data` (so chained `outer.inner.X` reads the inner
*module value* and the chain stays drillable), then the child scope's
type-side `bindings.types` via `Scope::resolve_type` — surfacing the type in the value
channel's `Type` arm so type-position consumers (e.g. a LET-RHS routing
through dep-finish) see a first-class `KType` value. The resolved type is
the leaf's existing per-declaration `SetMember` handle; no new
node kind, no path field.

**Forward type aliases are position errors.** A top-level `LET Ty = Un; LET Un =
Number` rejects: the Type-classed `Un` token on the first LET's RHS resolves under
the same `idx < cutoff` chain gate as a value reference, and `Un` is not yet
visible at `Ty`'s position. A source-order alias (`LET Un = Number; LET Ty = Un`)
binds normally and writes through `Scope::register_type` to land in
`bindings.types`. Mutual recursion that genuinely needs a cycle is declared inside a
module body, whose announcement co-declares the group.

## Every definition-time site is gated to its binder's position

A definition-time type resolution is gated to the lexical position of the binder
that owns it: NEWTYPE / UNION field types, FN parameter
slots, and FN / MATCH / TRY return types all resolve against the chain at their
binder's cutoff (`classify_return_type` / `resolve_arm_return_contract` thread the
chain for the return-type sites). A field or parameter naming a type declared
later is a position error, the same rule the value language enforces.

A *deferred* return type — one that references a parameter, like a functor's
`-> :(TYPE OF er)` — is the one definition-time site with no forward-reference question to
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
  [`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs)
  (a match over the eleven-entry builtin map, re-run per call). A hit lowers to a resolved
  `KType` handle in the value channel's `Type` arm; a miss — a user-bound leaf — defers to the
  [`Carried::UnresolvedType`](../../src/machine/model/values/carried.rs) carrier over the
  surface `TypeIdentifier`, which
  preserves the parser-side name verbatim until the park-capable
  `Scope::resolve_type_identifier` consumes it. Runs at `KFunction::bind` time, which has no
  `Scope` in hand, so it is builtin-only and scope-independent.
- **Layer 2 — scope-bound resolution**, reached through
  [`Scope::resolve_type_identifier`](../../src/machine/execute/decide/resolve_type_identifier.rs), which takes the
  consumer's chain and returns the three-outcome
  `TypeResolution<KType>::{Done, Park, Unbound}`. It runs the elaborator on every call
  and caches nothing: interning in the
  [`TypeRegistry`](../../src/machine/model/types.rs) already makes a re-elaborated form
  yield the *same* handle. A `Done` passes a finalize check on every embedded user-type
  first — see [Strict admission rules](#strict-admission-rules) for the gate.
- **Layer 3 — the elaborator** in
  [`resolver.rs`](../../src/machine/model/types/resolver.rs). Resolves a
  *bare-leaf* `TypeName` against the scope into a `KType` handle, carrying the
  consumer's `LexicalFrame` chain so each candidate is gated by `idx < cutoff`. The
  declaration window is consulted before the binding tables, and answers a co-declared
  name by who is asking: a declarator (its own name, or a co-announced sibling) takes a
  relative `TypeNode::Sibling` reference, a consumer parks until the window seals and
  then takes the member's absolute handle. Otherwise an *earlier still-finalizing* binder parks via
  `TypeResolution::Park(producers)`, a later-than-the-consumer binding is a position
  error, and a builtin name falls back to the builtin table through
  [`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs) —
  the single owner of that fallback on the resolution path. Parameterized shapes
  (`:(LIST OF X)`, `:(MAP K -> V)`) sub-Dispatch through the standalone dispatcher
  rather than recursing here, so the only recursion is the sibling-result reduce.
  FN-signature, NEWTYPE/UNION field-type, and per-call return-type dep-finishes
  reduce to this leaf walk; see
  [execution/name-placeholders.md § Dispatch-time name placeholders](../execution/name-placeholders.md#dispatch-time-name-placeholders)
  for the parking integration.
- **Layer 4 — bare-leaf dispatch ingress** in
  [`resolve_type_identifier.rs`](../../src/machine/execute/decide/resolve_type_identifier.rs).
  The bare-`Type` token call sites — the dispatcher's `BareTypeLeaf` fast lane and the
  keyworded splice walk's eager name-resolve pass — call
  [`Scope::resolve_type_identifier`](../../src/machine/execute/decide/resolve_type_identifier.rs)
  directly, the same memoized, park-capable bridge (Layer 2) every compound type form
  uses. It returns `TypeResolution<KType>::{Done, Park, Unbound}`: `Done` carries the
  bridge's cached `KType` handle on the value
  channel's `Type` arm — a `KType` is a `Copy` handle, so there is no reach to carry beside it;
  `Park` lets a leaf naming an earlier still-finalizing binder park
  on its producer and re-resolve on wake; `Unbound` is a miss. See
  [Bare-leaf type-name carrier](#bare-leaf-type-name-carrier) below for
  the downstream consumers.
- **Layer 5 — surface-form-survives-bind carrier** in
  [`carried.rs`](../../src/machine/model/values/carried.rs).
  [`Carried::UnresolvedType` / `Held::UnresolvedType`](../../src/machine/model/values/carried.rs)
  preserve the parser-side `TypeIdentifier` verbatim for bare-leaf type names not in the
  builtin table — so diagnostics quote the user's identifier exactly as written rather than
  the elaborated canonical form, and no type handle ever denotes an unresolved name. The
  carrier never reaches the dispatch
  predicates: `Scope::resolve_type_identifier` consumes and replaces it. See
  [Bare-leaf type-name carrier](#bare-leaf-type-name-carrier) for the consumers.

## Binding-map partition

Type bindings live in a separate map from value bindings — the type-side
slice of the [lookup → admit protocol](lookup-protocol.md)'s Layer 2.
The [`Bindings`](../../src/machine/core/bindings.rs) façade owns four
maps under one `RefCell`: `data` for values, `functions` for registered overloads,
`operators` for the operator-group registry, and `types` for type-name → `KType`
handles. An in-flight dispatch task holds no map of its own — it claims a pending
arm of the slot it will resolve into, in whichever of those three tables that is.

`types` and `data` are **different universes, and a name's token class decides which one it
belongs to** — a Type token names something that can type a field, a value token names
something a field can hold. The enforcement is the **key types**: `types` keys by
`TypeSymbol` and `data` by `ValueSymbol`, classified newtypes over a label `Symbol` minted
only from text of their own token class
([label-interning.md § Classified label vocabulary](../label-interning.md#classified-label-vocabulary)).
The two classify disjoint text, so a name reaching both maps is unrepresentable rather than
rejected, and a write door needs no cross-kind probe.

The rule is a runtime answer at exactly one place: the **text→symbol declaration seam**,
where a binder's source text is classified into the channel it binds into. A name that
will not classify there raises a `ShapeError` naming the partition it crosses — a value
token entering the type channel ("`int_ord` is a value token, so it names a value"), a Type
token entering the value channel ("`IntOrd` is a Type token, so it names a type"). Past
that seam every key carries its own class. A keyword-class name (all-uppercase, no
lowercase — `PRINT`) classifies into neither bindable channel: **nothing binds to a
keyword**. Keyworded dispatch registration is unaffected, because it labels a bucket in
`functions` rather than binding a name.

Two consequences follow, and both are load-bearing elsewhere in this tree:

- **A name is committed to one universe before any value reaches it.** A *cross-kind
  collision* — the same name in both maps — is therefore unconstructible, with no runtime
  check standing behind it.
- **A parameter's name picks its universe, not its argument.** A `:Type`- or
  `:Signature`-slotted parameter carries a type-language value, so it must spell as a Type
  token (`Ty`, `Er`); a module-valued parameter carries a *value*, so it must spell snake_case
  (`er`). Handing a module to a Type-token parameter is a bind-time error, not a silent
  crossing.

**SIG value slots live off the binding map.** A SIG body records each `VAL <name> :<Type>`
slot's declared type into a **slot collector** on the decl scope
([`Scope::sig_slot`](../../src/machine/core/scope.rs) / `sig_value_slots`), keyed by the slot's
value name — never into the `types` map. That collector
is a **schema in progress, not a binding universe**: no name resolves against it and it carries
no `BindingIndex`. The `types` map holds only the SIG's genuine type members (`TYPE <Name>`
abstract, `LET <Name> = <Type>` manifest), so the value-token→`types` gate needs no exemption
and the decl scope's `Bindings` is an ordinary [`Bindings::new()`](../../src/machine/core/bindings.rs)
([`Scope::child_under_sig`](../../src/machine/core/scope.rs)). At SIG finish the member map and
the slot collector project once into the signature's stored
[`SigSchema`](../../src/machine/model/types/sig_schema.rs) — `is_abstract_sig_member` reads
representation to split abstract from manifest members, and the slot collector supplies
`value_slots` directly, with no per-read reprojection.

Token-class-driven lookup at the resolver
decides which map to consult — Type-class tokens consult `types`,
identifier tokens consult `data`. Builtin type names *and* `LET Ty =
Number`-style aliases live in `bindings.types` as `KType` handles by value,
reachable through
[`Scope::resolve_type`](../../src/machine/core/scope.rs) as the same
handle as the builtin.

NEWTYPE / UNION / Result / SIG declarations are all single
type-namespace writes: each installs only its identity-bearing `KType` handle into
`bindings.types` (see
[type-only nominal install](user-types.md#type-only-nominal-install)), so
`bindings.data` carries **zero** type carriers — the type-language /
value-language partition is total. There is no value-side schema or signature
carrier; construction reads the schema straight off the identity, and a
signature value is synthesized on demand from the type entry. SIG's single
`KType::Signature { schema, .. }` variant serves both its constraint
role and its value role, so it installs one type-side identity like every other
nominal binder. `MODULE` is the one declarator that writes the *value* side: a
module is a value, so it binds into `bindings.data`
([modules.md § First-class modules](modules.md#first-class-modules)). No binder
dual-writes `(bindings.types, bindings.data)` — the cross-kind exclusion commits
each name to exactly one map.

[LET routing in `let_binding`](../../src/builtins/let_binding.rs) branches on the
binder name's token class, and each branch admits exactly the RHS kinds its map can
hold. A **Type-class LHS** admits an RHS only if it carries type-language identity:
any value-channel `Type` arm (struct / union / Result / signature identities all flow
raw as `&KType`). A
module RHS is refused with the snake_case respelling — a module is a value, and the
Type-token namespace names what can type a field. A **function** RHS is refused for
the same reason: a function is a value whatever it returns, so a module-returning FN
binds value-side like any other (see
[functors.md § Application and binding](functors.md#application-and-binding)), and
`bindings.types` holds no callable. The combined statement form is refused at its own
overload — `LET <Name> = FN …` is a `ShapeError` carrying the snake_case respelling,
before any definition is built. Any other object rejects with
`KErrorKind::TypeClassBindingExpectsType`. Every admitted RHS — struct / union /
Result, and signature —
routes through `register_type` (type-only): the schema or `&Signature`
rides the `KType` identity, so a plain `types` write preserves dispatch identity
without a value-side copy. A `LET Sortable = Ordered` signature alias therefore dispatches
identically to the original, with no separate nominal-install path.

The partition is one-way and total against type-language carriers. A
**value-classified LET** (lowercase-leading binder name) rejects **any** type
RHS at the LET site with a `ShapeError` redirecting the user to a
Type-classified name: every value-channel `Type` arm (struct / union /
Result / signature / builtin type, including the `UnresolvedType` parser-form
carrier). A `KObject::Module` is *not* rejected — a module is a value, and a
value-classified name is exactly where it belongs. A type therefore binds only under
a Type-classified name; construction
names the type directly (`Point {…}`) or through a Type-classified alias
(`LET Pt2 = Point` then `Pt2 {…}`), never a value-classified one. Combined with
the Type-class LET admission above, this makes `bindings.types` the single home
for every type identity, so `bindings.data` is unconditionally free of
type-language carriers (a module value is not one — it is a value) and the
value-side and type-class lookup paths never both
find one under the same name. This exclusion is **structural**, not just a
LET-site convention: the [`Bindings`](../../src/machine/core/bindings.rs) write
paths themselves reject a cross-kind collision — a value bind whose name is a
committed type, or a type register whose name is a committed value, is a
`Rebind` — so no bind site (LET, VAL, NEWTYPE, UNION, MODULE, SIG, RECURSIVE, or a
`USING` replay) can land a name in both maps. The [token-class rule](tokens.md)
defines the Type-class shape (uppercase-leading plus at least one lowercase
letter); the LET-site classification guard (which map) lives in
[`let_binding`'s `body`](../../src/builtins/let_binding.rs), and the
mutual-exclusion guard (one map, never both) is the cross-kind check the value
and type write paths run — see
[lookup-protocol.md § Layer 2](lookup-protocol.md#layer-2--bindings-per-scope-lookup).

The value-side ATTR walker and ascription's type-member sweep both walk
`bindings.types`, classifying each Type-class entry by representation via
`is_abstract_sig_member` (`abstract_members_of` / `manifest_type_members_of`),
so a SIG's `TYPE` abstract members and `LET <Name> = …` manifest members resolve
uniformly under one model.

## Bare-leaf type-name carrier

Bare-leaf type names that aren't in
[`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs)'s builtin
table (`Point`, `Ordered`, `MyList`) are lowered by
[`ExpressionPart::resolve_for`](../../src/machine/model/ast.rs) onto the dedicated
[`Carried::UnresolvedType`](../../src/machine/model/values/carried.rs) carrier — holding
the surface `TypeIdentifier` — rather than a resolved `KType` handle in the `Type` arm, so
no type handle can denote an unresolved name.
The carrier preserves the parser-side name for diagnostics and for
consumers that want the user's surface identifier verbatim. Both the `UnresolvedType`
carrier and a fully-resolved type report
`ktype() = KType::of_kind(ProperType)`, so the slot's dispatch position is identical —
whether the surface form already lowered to a concrete `KType` handle at bind time
or is still in parser-form is an internal detail.

Downstream consumers branch on the two arms, handling the `UnresolvedType` carrier where
a bare user name can still be pending:

- the shared
  [`require_bare_type_name`](../../src/machine/core/kfunction/action.rs)
  helper (used by the nominal binders that read their name from a `KObject::Record`
  type cell), which renders either an unresolved name or a resolved type;
- [ATTR's `body_type_lhs` and `read_field_name`](../../src/builtins/attr.rs);
- [`let_binding`'s name slot](../../src/builtins/let_binding.rs), which
  runs a primitive/container blocklist over the `Type` arm and
  routes to `register_type` for type-valued RHSes.

The single-part bare-`Type` lookup that those consumers' siblings need is
folded into the dispatcher's `BareTypeLeaf` fast lane
([`decide/single_poll.rs`](../../src/machine/execute/decide/single_poll.rs)),
which calls the park-capable
[`Scope::resolve_type_identifier`](../../src/machine/execute/decide/resolve_type_identifier.rs)
bridge directly — the same bridge the keyworded splice walk's eager
name-resolve pass calls
([`decide.rs`](../../src/machine/execute/decide.rs)).
On a resolved leaf its `TypeResolution::Done(KType)` surfaces the interned
`KType` handle in the value channel's `Type` arm for every type-only nominal — struct / union /
Result *and* signature; on an earlier still-finalizing binder it parks; on a
miss it surfaces `Unbound`. The ladder consults **only** the type universe: the token-class
partition commits a Type token to `types`, so a Type-token leaf can hold no value for the
resolver to layer a sharper miss over, and a leaf naming no type is an ordinary unknown-name
miss. What reaches a value from type position is `TYPE OF` (see
[modules.md § Modules in type position](modules.md#modules-in-type-position-type-of)).

FN's deferred return-type slot is parsed at definition time via
[`extract_return_type_raw`](../../src/builtins/fn_def/return_type.rs), which reads either
a resolved `KType` handle or the `UnresolvedType` carrier for a bare leaf —
and branches on the unresolved case to pick the `TypeExpr` carrier (see
[fn_def/return_type.rs](../../src/builtins/fn_def.rs)). At call time the body executor
[`run_user_fn`](../../src/machine/core/kfunction/exec.rs) elaborates that `TypeExpr` carrier
inline against the per-call child scope (`elaborate_type_expr`), where the param install has
already finalized every parameter-name identity; type-denoting parameters themselves bind via
`register_type` from an already-resolved type argument, so there is no transient identity
elaboration at the bind site. The sole bare-leaf resolution site for dispatch transport
is the
[`Scope::resolve_type_identifier`](../../src/machine/execute/decide/resolve_type_identifier.rs)
bridge, which surfaces the resolved `KType`. Bare leaves resolve through the same
gate and parking discipline as compound type forms — there is no separate
synchronous bare-leaf path.

Every `KType` flowing through dispatch is fully elaborated — there is no
surface-name carrier variant inside `KType` itself.

## Strict admission rules

**Parking pre-empts admission.** Before any candidate is consulted,
[`resolve_dispatch`](../../src/machine/execute/decide/resolve_dispatch.rs) scans the
per-dispatch-poll `bare_outcomes` cache and parks on the producers behind every `Parked`
bare-name part — so admission always decides against landed facts, and which overload wins
never depends on drain order. Two slot kinds are exempt, both owned by a binder form's own
machinery and both statically known from the expression's cached spec-table facts
([`KExpression::binder_name_slot`](../../src/machine/model/ast.rs), off
[`BinderSpec::name_slot`](../../src/machine/model/binder.rs)):

- the **declared-name position** — the slot owns the name (`x` in `LET x = …`), so an inner
  shadowing binder must not wait on a same-named outer binder still in flight (its own claim
  is already invisible to it by the exclusive visibility cutoff);
- a binder form's **`Type`-token operands** (`NEWTYPE M = Alias`, a combined form's return
  type) — the binder body resolves these through the declaration-window / type-resolution
  protocol, which answers a declarator's reference to a co-declared sibling without waiting.

[`signature_admits_strict`](../../src/machine/execute/decide/resolve_dispatch.rs)
then admits a candidate signature against the expression by walking slot/part
pairs and consulting the cache. The admission rule per cache entry on a
bare-name part:

| Cache entry              | Admission rule                                                                   |
|--------------------------|----------------------------------------------------------------------------------|
| `Resolved(obj)`          | Admit iff [`KType::accepts_part`](../../src/machine/model/types/ktype_predicates.rs) accepts `Future(obj)`. A wrong carried type strict-rejects rather than tentative-admitting into a bind-time `TypeMismatch`. |
| `Parked` / `Unbound`     | Reject: a name that has not landed satisfies no typed value slot. An `Unbound` name's precise `UnboundName` diagnostic rides the relaxed pass's `Dead` lean; `Parked` reaches admission only on a pre-scan-exempt slot, where the reject leaves the pick to the binder overloads' shape-only slots. |
| `None` (non-bare part)   | Fall back to shape-only `arg.matches(part)`.                                     |

A producer error never reaches this table, and no fourth row is hiding: the
cache-building ladder is **total**, so every bare-name part lands on one of the
three rungs above. A producer's error is not a rung — it reaches this consumer
through the park the harness installs, ruled on at wiring time rather than
probed here
([classify-and-apply.md](../execution/classify-and-apply.md)).
Cycle detection is likewise deferred — the cache carries no consumer id — so
neither state is a `Resolution` variant admission must screen.

**Binder declaration slots bypass the cache.** A slot typed `KType::Identifier`
or `KType::OfKind(KKind::ProperType)` owns the name (`x` in `LET x = …`, `Ty` in
`NEWTYPE Ty = …`), so admission must be shape-only regardless of whether
the name happens to be bound elsewhere. A `SigiledTypeExpr` or `RecordType`
part still admits speculatively in such a slot — it sub-dispatches to a
type-side carrier downstream. The same shape-only-on-binder-slot rule covers
`KExpression` slots: the slot owns its body, not a name lookup.

Elaboration carries no cache tier. Bind-time builtin lowering
([`ExpressionPart::resolve_for`](../../src/machine/model/ast.rs) →
[`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs))
re-runs the eleven-entry builtin match per call — the match is cheap, and a shared
table would be added back only if profiling shows it hot. Scope-bound resolution
is the same: interning in the [`TypeRegistry`](../../src/machine/model/types.rs)
already makes a re-elaborated form yield the *same* handle, so a per-scope memo
would buy only the elaborator walk while owning an invalidation question.

- [`Scope::resolve_type_identifier`](../../src/machine/execute/decide/resolve_type_identifier.rs) runs the
  elaborator against `self` on every call and returns the three-outcome
  `TypeResolution<KType>::{Done(KType), Park(Vec<NodeId>), Unbound(String)}` — the
  handle alone, with no stored reach beside it, since a `KType` owns all its content.
  A `Done` passes a **finalize gate** first: every user-type referenced by the
  result must be fully finalized (no type-side placeholder left in the owning scope)
  or the outcome becomes `Park(producers)`. The walk is top-level only — a declaration
  window seals every member together, so a finalized `Foo`
  guarantees every user-type embedded in `Foo`'s payload is also finalized.

Consumers that need the scope-resolved identity —
[`val_decl::body`](../../src/builtins/val_decl.rs)'s structural
carrier path and its post-dep-finish, and
[`fn_def::body`](../../src/builtins/fn_def.rs)'s return-type
elaboration — go through `Scope::resolve_type_identifier`. NEWTYPE's bare-leaf
user-bound repr path keeps the simpler `Scope::resolve_type` lookup (it's
intentionally non-park-aware: an unresolvable repr is a hard error, not a
forward reference). A type-denoting FN parameter binds its already-resolved type
argument directly via `register_type` in
[`run_user_fn`](../../src/machine/core/kfunction/exec.rs), so the per-call
type-side bind needs no scope re-resolution and cannot park.
