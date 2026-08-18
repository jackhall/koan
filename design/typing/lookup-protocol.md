# The lookup → admit protocol

Every dispatch site and every name-resolution site in koan threads the
same three layers:

1. **`Scope` finds the ancestor** by walking the scope chain against
   the consumer's `LexicalFrame`.
2. **`Bindings` finds the entry** in that ancestor's per-kind map and
   gates it against the consumer's `chain_cutoff` via the
   `visible(b.idx < c)` predicate — one rule across the value and type
   languages, with no per-binding exemption.
3. **`KType` predicates accept or reject** the candidate against the
   slot's declared shape.

This page is the single named owner of the protocol. The participants
are correctly distributed across four source files — chain walk,
per-scope map lookup, and type-shape admission are each at their right
level — so wrapping them in a `core::lookup` module *adds* coupling
without dissolving the through-traffic. The 2026-05 candidate
analysis confirmed this (Pass 15, Δ +5.46 even with paired doc
consolidation). The seam exists only at the doc level: every typing /
dispatch doc has to mention the protocol because every operation in
its concern threads it. See
[design/README.md § Foundation vs seam](../README.md#foundation-vs-seam)
for the test that distinguishes the two patterns.

## Layer 1 — `Scope` chain walk

[`Scope`](../../src/machine/core/scope.rs) walks ancestor scopes once
per lookup, threading the consumer's
[`LexicalFrame`](../../src/machine/core/lexical_frame.rs) chain so each
ancestor receives the right `chain_cutoff`. Three entry points,
matching the three lookup kinds:

- [`Scope::resolve_with_chain`](../../src/machine/core/scope.rs) —
  value-name lookup. Per-ancestor calls
  [`Bindings::lookup_value`](../../src/machine/core/bindings.rs) and
  returns the first visible hit.
- [`Scope::resolve_type_with_chain`](../../src/machine/core/scope.rs) —
  type-name lookup. Per-ancestor calls
  [`Bindings::lookup_type`](../../src/machine/core/bindings.rs).
- [`Scope::resolve_dispatch`](../../src/machine/execute/decide/resolve_dispatch.rs)
  — function-bucket lookup. Per-ancestor calls
  [`Bindings::lookup_function`](../../src/machine/core/bindings.rs)
  and decides the scope's contribution from the returned
  [`FunctionLookup`](../../src/machine/core/bindings.rs): a visible pending
  producer parks the scope, otherwise the finalized overloads go to
  [`OverloadBucket::pick_strict`](../../src/machine/execute/decide/resolve_dispatch.rs)
  for the per-overload admit pass. The innermost scope to reach a terminal
  decision wins (see
  [scheduler.md § In-walk dispatch precedence](scheduler.md#in-walk-dispatch-precedence)).
- [`Scope::resolve_operator_group_delivered`](../../src/machine/core/scope/resolve.rs)
  — operator-group lookup. Per-ancestor calls
  [`Bindings::lookup_operator_group`](../../src/machine/core/bindings.rs) with
  the chain's operator probe and lifts the first visible hit into a delivery
  envelope at the scope that declared it. The probe is the
  sorted-joined unique operators of a `Slot (Keyword Slot)+` chain (parse-cached
  on the [`KExpression`](../../src/machine/model/ast.rs)); a module installs the
  group under every nonempty subset of its operators, so any subset used in one
  chain resolves in one visible hit and a cross-group mix simply misses.

[`Scope::resolve`](../../src/machine/core/scope.rs) is the chainless
shorthand — it reads as "see everything" and is reserved for test
fixtures and builtin-registration paths. Production dispatch always
threads a chain.

## Layer 2 — `Bindings` per-scope lookup

[`Bindings`](../../src/machine/core/bindings.rs) owns the per-scope
maps — `data` (values), `types` (type-name → `&KType`), `functions`
(registered overloads), `operators` (operator probe → the sealed carrier of a
region-hosted [`OperatorGroup`](../../src/machine/model/operators.rs)). An
in-flight binder gets no map of its own: it claims a *pending arm* of the slot it
will resolve into, so each of the three tables holds bound and pending state in
one entry ([execution/name-placeholders.md § A placeholder is a pending arm of
the slot it resolves into](../execution/name-placeholders.md#a-placeholder-is-a-pending-arm-of-the-slot-it-resolves-into)),
and a [`BindKind`](../../src/machine/core/bindings.rs) — `Value` or `Type` —
picks which table a name-keyed claim lands in. The `data`/`types`
split is **structural, not conventional**, and it is enforced twice over. First by
**token class**: `Bindings::partition_guard` refuses a value token entering `types` and a
Type token entering `data`, so a name is committed to one universe by its spelling alone,
before any binding reaches it (see
[tokens.md § Token class is a binding rule](tokens.md#token-class-is-a-binding-rule-not-just-a-lexical-one)
and [elaboration.md § Binding-map partition](elaboration.md#binding-map-partition)). Second by
**cross-kind exclusion**: every value write path rejects a
name already committed to `types`, and every type write path rejects one
already in `data` (a `Rebind` either way). The token-class gate makes the second
unreachable — a name that cannot cross cannot collide — so cross-kind exclusion is a
belt-and-braces backstop rather than a routine gate: no map mixes classes, since a SIG body's
value slots live off the binding map in the decl scope's slot collector.
One name can never hold both a
value and a type, and a lookup can never return the wrong kind. `bind_value`
and `register_function` finalize their own claim by overwriting the slot holding
it, and a claim of one kind lives in a table a write of the other kind never
touches — so a value bind can never satisfy an in-flight type producer's claim,
nor the reverse.

- [`Bindings::lookup_value`](../../src/machine/core/bindings.rs)
  reads the one `data[name]` slot, whose arm is bound xor pending. Returns
  `Some(NameLookup::Bound(&KObject))` for a finalized visible binding,
  `Some(NameLookup::Parked(EdgeId))` for a still-running visible
  producer — the claim edge that binder installed, which the caller parks by
  wiring its own edge off — or `None` on a miss, the caller keeping on
  walking ancestors. The terminal unbound disposition is not a lookup variant:
  it is materialized one level up on the resolution path, where
  [`Resolution`](../../src/machine/execute/decide/resolve.rs) is the
  merge point that adds the `Unbound` state. The producer's standing is not read
  here at all — a lookup runs on a read-only view, and classifying a producer is
  the install door's answer at wiring time
  ([scheduler-library.md § The install door](../scheduler-library.md#the-consumer-api)),
  so neither a producer error nor a cycle is a state the cache carries. A
  same-name type-side claim is invisible here — it lives in `types`, and belongs
  to the type language.
- [`Bindings::lookup_type`](../../src/machine/core/bindings.rs) is the
  type-side symmetry: reads the one `types[name]` slot, preferring its bound arm
  over its pending one, surfacing the result as the same
  [`NameLookup`](../../src/machine/core/bindings.rs) shape instantiated for the type
  channel (`NameLookup<KType>`) — `Bound(KType)`, `Parked(EdgeId)`, or `None`. Every
  single-scope lookup — value and type alike — shares this one
  bound-or-parked-or-miss shape; the value channel instantiates it at the resting
  carrier (`NameLookup<SealedValue>`), which fuses the value with its proven reach.
  There is no type-side
  carrier twin: a `KType` is a `Copy` handle owning all its content, so a type lookup
  hands the handle back by copy. The
  finalize gate that must park on an
  in-flight type producer even after a seal pre-installs the name's identity
  into `types` — the slot's `BoundWithPending` arm — reads the pending claim's
  edge directly through
  [`Bindings::type_placeholder_producer`](../../src/machine/core/bindings.rs),
  bypassing the bound-first preference. Declaration identity itself is not read
  here: it is decided at the install door by
  [`Bindings::try_register_type_upsert`](../../src/machine/core/bindings.rs), which
  compares the installing [`Installer`](../../src/machine/core/bindings.rs) against the
  stored entry's. What this gate resolves is in-flight status, not identity: a nominal
  member named by a relative `Sibling` reference is in flight iff the scope carrying the
  very group window that reference resolves against still holds a pending arm for
  the name
  ([resolve_type_identifier.rs](../../src/machine/execute/decide/resolve_type_identifier.rs)).
  The window match is what stops a same-named in-flight declaration of a
  different type from capturing the reference; a SIG-declared or abstract slot,
  whose type records its declaring scope id, is matched by that id instead.
- [`Bindings::lookup_member`](../../src/machine/core/bindings.rs) is the ATTR
  module/signature read: one classified pass over the scope's *own* `data` then
  `types`, returning a `MemberResolution::Value(&KObject)` or
  `MemberResolution::Type(&KType)` (or `None`). It is deliberately module-own —
  it consults neither the builtin root nor lexical ancestors, so `m.<non-member>`
  is a missing member, not a fall-through to a builtin or outer type — and the
  cross-kind exclusion keeps a name from matching both arms.
- [`Bindings::lookup_function`](../../src/machine/core/bindings.rs)
  surfaces both kinds of slot in one pass over the one bucket, as a
  `FunctionLookup` struct: `overloads` is the visibility-filtered sealed slots of
  `functions[key]` (possibly empty) and `pending` is the earliest-index visible
  pending slot in that same bucket (an in-flight FN binder to park on, if any).
  The two are
  returned together — a bucket may hold a finalized overload *and* an in-flight
  pending sibling at once — so the scope walk decides pending-vs-finalized
  precedence with both in hand rather than the lookup shadowing one. A scope
  contributes nothing to the walk when `overloads.is_empty() &&
  pending.is_none()`.
- [`Bindings::pending_function_stored`](../../src/machine/core/bindings.rs) is
  that lookup's pending-only peer, for a caller that reads nothing but the
  in-flight claim — the operator chain's park probe, which asks two keys of every
  ancestor scope per operator. It answers from the same bucket by the same
  earliest-visible-index rule, written once so the two entry points cannot name
  different claims, and copies nothing out: a `ProducerId` is a plain edge name,
  where `overloads` has to duplicate its sealed carriers to let the candidate walk
  run outside the `tables` borrow.

[`Bindings::lookup_operator_group`](../../src/machine/core/bindings.rs) is the
operator-side instance: it consults the `operators` map for the chain's probe
and returns a bit-copy of the visible entry's sealed
[`OperatorGroup`](../../src/machine/model/operators.rs) carrier,
or `None` so the caller keeps walking. Unlike the value/type/function maps it
holds no in-flight placeholder arm — operator registration is not a parkable
producer — so the lookup is a single map read gated by `visible`.

The visibility predicate is one line —
[`visible(b: BindingIndex, chain_cutoff: Option<usize>)`](../../src/machine/core/bindings.rs)
— shared across all of them. `b.idx < c` is the strict gate governing the chain walk:
every binder — value and type alike — gates its references against its own
lexical position, so a forward reference is a position error in both languages.
(The one lookup that precedes the gated walk is the root-first builtin short-circuit —
see [The immutable root and unshadowable builtins](#the-immutable-root-and-unshadowable-builtins);
it applies only to `idx == 0` builtins, which a forward reference can never name.)
Mutual recursion of two or more nominal types is expressed by declaring them in one
module body, whose announcement scopes the co-declared group to that body rather than
bypassing the cutoff (see
[user-types.md § Mutual recursion](user-types.md#mutual-recursion--the-module-body-announcement)).

## Layer 3 — `KType` predicates

[`KType` predicates](../../src/machine/model/types/ktype_predicates.rs)
decide whether a candidate's carried type satisfies the slot's
declared shape. The three predicates partition the work by where the
check fires:

- [`KType::accepts_part`](../../src/machine/model/types/ktype_predicates.rs)
  — admission predicate. Tests an `ExpressionPart` (typically a
  `Future(obj)` for a resolved bare-name slot) against a declared
  slot type during dispatch admission. The strict-only admission
  rules table lives at
  [elaboration.md § Strict admission rules](elaboration.md#strict-admission-rules);
  the cache it consumes is built once per dispatch poll and shared
  between the strict admit pass and the post-pick splice walk.
- [`KType::is_more_specific_than`](../../src/machine/model/types/ktype_predicates.rs)
  — specificity ranking. Ranks two slot types when multiple overloads
  admit the same call, used by `ExpressionSignature::most_specific` to
  break ties. Concrete carrier types outrank `KType::Any`; a user-type
  identity outranks the `OfKind(KKind)` family kind of its own family. The full ranking
  rules and variance behavior live at
  [ktype/parameterization-and-variance.md § Variance](ktype/parameterization-and-variance.md#variance) and
  [user-types.md § Specificity stratification](user-types.md#specificity-stratification).
- [`KType::matches_value`](../../src/machine/model/types/ktype_predicates.rs)
  — runtime content check. Walks a runtime value's contents against a
  declared type at an ascription boundary (FN return, FN argument,
  `LET`). This is the only predicate that walks contents; the other
  two read carrier-type metadata in O(1).

The dispatch-admission glue is
[`signature_admits_strict`](../../src/machine/execute/decide/resolve_dispatch.rs),
which walks slot/part pairs and consults the per-dispatch-poll
`bare_outcomes` cache — the strict-admission rules table at
[elaboration.md § Strict admission rules](elaboration.md#strict-admission-rules)
spells out which `Resolution` arms admit via `accepts_part`, which
admit shape-only, and which strict-reject. [`OverloadBucket::pick_strict`](../../src/machine/execute/decide/resolve_dispatch.rs)
wraps the filter-then-`most_specific` dance over a single scope's
visibility-pre-filtered bucket.

### Overload identity

Two overloads in one bucket are **indistinguishable** when they have the same element
shape and a type-equal `Argument` slot at every position. That is the definition-time
duplicate gate
([`ExpressionSignature::indistinguishable_from`](../../src/machine/model/types/signature.rs)),
raising `KErrorKind::DuplicateOverload` from the bind door
([`bindings.rs`](../../src/machine/core/bindings.rs)) rather than admitting a pair no
call could ever resolve.

The gate is the exact complement of the specificity tournament above, and lives
adjacent to it for that reason: per-slot type equality makes every mutual
`is_more_specific_than` probe false, so such a pair ties as `Equal` on every call and
poisons the bucket with unresolvable ambiguity. Parameter names do not participate —
they are not part of the call. Neither do return types: dispatch selects on argument
slots alone, so two definitions differing only in their declared return are the same
overload, and the second is a duplicate. (Return types do participate in a function
*type*'s identity, which is a different relation — see
[ktype/slots-and-signatures.md](ktype/slots-and-signatures.md).)

## The immutable root and unshadowable builtins

The builtins live once, in a distinctly-typed run-global root
([`ScopeKind::Root`](../../src/machine/core/scope.rs)) that holds them and accepts no
user bindings. [`unseeded_scopes`](../../src/builtins.rs) allocates that root together with
a mutable `RunScope` child of it, and [`seed_builtins`](../../src/builtins.rs) registers the
builtins into the root, so top-level Koan bindings land in the `RunScope` and the root stays
builtin-only. The two are split because seeding takes the run frame's
[type registry](type-registry.md): the run frame is established against the `RunScope` first,
and its registry is what the builtins are seeded against. Every [`Scope`](../../src/machine/core/scope.rs)
carries a direct `root` handle (`None` iff it *is* the root), so any frame, however deeply
nested, reaches the builtins in one hop through
[`Scope::root_scope`](../../src/machine/core/scope.rs) rather than walking `outer` to the
chain's tail. A builtin is tagged [`BindingIndex::BUILTIN`](../../src/machine/core/bindings.rs)
(`idx == 0`); it stays visible from any depth because the root is off the lexical chain (its
`chain_cutoff` is `None`, so every entry in it is visible), not through any `idx == 0`
visibility exemption.

**Builtins are immutable and unshadowable.** A user binding that collides with a builtin is
a `Rebind` at any scope depth — never a shadow, never a merge:

- A user *type* (nominal `UNION` / `SIG` / `NEWTYPE` / `RECURSIVE`
  declaration, or a SIG-body `TYPE` abstract / `LET <TypeName> = …` manifest type member)
  naming a builtin type is
  rejected — [`register_type_upsert`](../../src/machine/core/scope.rs) and
  [`register_user_type_delivered`](../../src/machine/core/scope.rs) consult
  [`Bindings::has_builtin_type`](../../src/machine/core/bindings.rs) on the root.
  Builtins are unshadowable in *either* channel: a value bind colliding with a
  committed type name is a `Rebind` too, through the same cross-kind exclusion.
- A user *FN* overload whose untyped signature key collides with a builtin
  dispatch bucket is rejected rather than joining it —
  [`Scope::register_function`](../../src/machine/core/scope.rs) consults
  [`Bindings::has_builtin_function`](../../src/machine/core/bindings.rs). An operator's
  bodies are function overloads, so they meet this same gate. User-vs-user
  overloads and cross-scope shadowing are unaffected — only an `idx == 0` builtin entry gates.
- The **operator registry** is the one table this rule does not reach: a builtin
  group is a chaining default, not an unshadowable claim on its symbols, so a scope
  that registers a probe the builtins already seed wins inside itself
  ([operators.md § Shadowing is type-gated](../operators.md#shadowing-is-type-gated-not-forbidden)).

Because a builtin can never be shadowed, a builtin entry is authoritative: it is resolved
root-first, in one hop through the direct reference, before the Layer-1 ancestor walk — the
constant-time path for the hottest names (operators, `PRINT`, dispatch primitives).
[`Scope::resolve_type_with_chain`](../../src/machine/core/scope/resolve.rs) returns a builtin
hit directly; [`Scope::resolve_dispatch`](../../src/machine/execute/decide/resolve_dispatch.rs)
consults the root's builtin bucket first and returns its decision when terminal. Each is gated
on the `idx == 0` [`has_builtin_*`](../../src/machine/core/bindings.rs) predicate, so a
non-builtin name finds nothing in the root and falls through to the Layer-1 chain walk with its
innermost-wins precedence intact — for dispatch, a non-terminal root decision likewise falls
through unchanged, so the short-circuit never overrides an inner scope.
[`Scope::resolve_operator_group_delivered`](../../src/machine/core/scope/resolve.rs) takes no
such consult at all: its walk is innermost-wins all the way, so the root's builtin groups are
simply found last.

## Why this is a foundation, not a seam

A *seam* is a contract restated across docs because no source file
owns it — the per-call region protocol was a seam that got a single
canonical doc owner, while the nominal dual-write was a seam *dissolved*
outright by folding each binder's two entries into one `KType` identity. A
*foundation* is a source file every operation in some concern *has*
to go through; it's correctly cited everywhere because the concept
the doc is explaining genuinely passes through that file. Wrapping a
foundation in a sub-module *adds* coupling without dissolving the
underlying through-traffic.

The lookup → admit protocol is a foundation. The three layers are at
their right level — chain-walk is scope-shaped, per-scope entry
lookup is bindings-shaped, type-shape admission is predicate-shaped —
and a `core::lookup/` module that wrapped them would force every
caller through an extra layer without removing any current
through-traffic. Pass 15 of the 2026-05 candidate analysis scored
this rewrite at Δ +5.46 even after consolidating the four-doc
restatements into one canonical page; the doc consolidation stands
on its own.

What each topic doc adds beyond this protocol:

- [elaboration.md](elaboration.md) — type-name resolution's
  five-layer pipeline (surface-form cache → scope-bound memo →
  elaborator → bare-leaf coercion → surface-name carrier) and the
  per-scope binding-map partition that separates type-name lookups
  from value-name lookups.
- [ktype/README.md](ktype/README.md) — `KType` variants, variance under the three
  predicates, container parameterization, and the overload-bucket
  visibility filter as it interacts with slot-specificity.
- [user-types.md](user-types.md) — the registry-interned nominal model
  (`SetMember` / `Sibling`), computed-SCC member identity,
  nominal-identity install through `Scope::register_type_upsert`, the specificity
  stratification for a concrete member handle vs `OfKind(KKind)` vs `Any`, and the
  module-body announcement for mutually recursive nominals.
- [execution/name-placeholders.md § Dispatch-time name placeholders](../execution/name-placeholders.md#dispatch-time-name-placeholders)
  — how forward references park on the pending arms of the `data` / `types` /
  `functions` tables and resume on producer finalize, plus
  the submission-time binder install that prevents `UnboundName` /
  `DispatchFailed` for not-yet-popped sibling binders.
