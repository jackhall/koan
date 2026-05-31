# User-declared nominal types

[`KType::UserType { kind, scope_id, name }`](../../src/machine/model/types/ktype.rs)
is the per-declaration identity tag for `STRUCT`, named `UNION`, and
`NEWTYPE`. The companion
[`KType::AnyUserType { kind }`](../../src/machine/model/types/ktype.rs)
wildcard accepts any `UserType` of the matching kind. Modules and
signatures live in their own KType variants —
[`KType::Module { module, frame }`](../../src/machine/model/types/ktype.rs)
for first-class module values,
[`KType::Signature(s)`](../../src/machine/model/types/ktype.rs) for
first-class signature values, and
[`KType::AbstractType { source_module, name }`](../../src/machine/model/types/ktype.rs)
for the abstract-type members opaque ascription mints — with
`KType::AnyModule` and `KType::AnySignature` as the matching wildcards
(see [modules.md](modules.md) for the carrier model).

[`enum UserTypeKind { Struct { fields }, Tagged { schema }, Newtype { repr }, TypeConstructor { schema, param_names } }`](../../src/machine/model/types/ktype.rs)
has a `surface_keyword()` accessor; every arm carries the declared type's
schema as its payload — record fields for `Struct`, tag→type maps for `Tagged`
/ `TypeConstructor`, the transparent repr for `Newtype`. A manual `PartialEq`
ignores all four payloads so wildcard / identity comparisons key on kind and
`(scope_id, name)` only; construction reads the payload off the identity stored
in `bindings.types` (see [Type-only nominal install](#type-only-nominal-install)).
The payload-empty `struct_sentinel()` / `tagged_sentinel()` constructors stand
in for any `Struct` / `Tagged` where only the kind matters — what an instance's
`.ktype()` synthesizes and what a cycle-close pre-install holds. The surface
names `"Struct"` / `"Tagged"` lower to
`AnyUserType { kind }` in
[`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs);
`"Module"` lowers to `KType::AnyModule` and `"Signature"` to
`KType::AnySignature`. [`scope.register_type`](../../src/builtins.rs)
agrees so the type-resolver and the builtin registry produce the same
wildcard carrier.

## Specificity stratification

Predicate arms
([`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs))
place `UserType { kind: K, .. }` strictly below `AnyUserType { kind: K }`
strictly below `Any` in `is_more_specific_than`, and `AnyUserType { kind }`
matches any `KObject::Struct` / `Tagged` of the matching kind. Each kind
ranks alongside the others with no per-kind branching at the dispatcher.
The module/signature variants follow the parallel stratification:
`KType::Module { .. }` ≺ `KType::AnyModule` ≺ `Any`, and
`KType::Signature(_)` ≺ `KType::AnySignature` ≺ `Any`. This is the
identity-and-wildcard slice of Layer 3 of the
[lookup → admit protocol](lookup-protocol.md); the predicate is the same
one every dispatch admit pass runs.

## Value carriers and the type / value partition

Instance carriers — [`KObject::Struct`](../../src/machine/model/values/kobject.rs)
and [`KObject::Tagged`](../../src/machine/model/values/kobject.rs) — carry
`(scope_id, name)` identity fields populated at finalize time via the
`scope as *const _ as usize` scheme `Module::scope_id()` uses. `ktype()` on a
`KObject::Struct` / `Tagged` reconstructs
`KType::UserType { kind, scope_id, name }`, synthesizing a payload-empty
`struct_sentinel()` / `tagged_sentinel()` kind — the dispatch identity needs
the `(scope_id, name)` discriminant, not the schema. Module and signature
values ride
[`KObject::KTypeValue(KType::Module { .. })`](../../src/machine/model/values/kobject.rs) /
`KObject::KTypeValue(KType::Signature(_))`; `ktype()` projects the carried
`KType` directly, so the identity is the carrier rather than a synthesized
shadow.

`bindings.data` holds only runtime instances. A value-position reference to a
nominal type token (passing `Outcome` to a constructor or ATTR call) synthesizes
[`KObject::KTypeValue(identity)`](../../src/machine/execute/dispatch/resolve_type_expr.rs)
on demand from the `bindings.types` entry via `coerce_type_token_value` — no
value-side schema carrier exists for struct / union / module / Result.

## Type-only nominal install

STRUCT / UNION-named / MODULE / Result finalize write **only** `bindings.types`:
each builds its schema-bearing `KType::UserType { kind, scope_id, name }`
(or `KType::Module { module, frame }`) identity and installs it through
[`Scope::register_type_upsert`](../../src/machine/core/scope.rs), which inserts
if absent and overwrites a `PartialEq`-equal entry (the cycle-close pre-install —
see [Cycle close](#cycle-close-for-mutually-recursive-nominals)), surfacing
`Rebind` on a genuine non-equal collision. The schema rides the identity, so
construction reads fields / variant types straight from the type entry; there is
no second-namespace write to keep in sync. The single-home invariant —
Type-classed name lookups go through `Scope::resolve_type` only — holds because
the identity *is* the only entry.

SIG is the lone exception: it still dual-writes via
[`Scope::register_nominal`](../../src/machine/core/scope.rs), `KType::SatisfiesSignature
{ sig_id, sig_path }` on the type side and `KTypeValue(KType::Signature(s))` on
the data side. Those two forms genuinely differ — a slot annotation
(`Er :OrderedSig`) means "any module satisfying OrderedSig," the constraint form,
while value-position lookups want the identity-bearing signature carrier itself,
which carries a live `decl_scope` the constraint can't reconstruct. Closing this
last dual-write is tracked in
[eliminate SIG's dual-write](../../roadmap/refactor/eliminate-sig-dual-write.md).

`LET <Type-class> = <module/sig/struct-value>` (e.g.
`LET IntOrdA = (IntOrd :| OrderedSig)`) installs the *original* type's identity
under the alias name rather than minting a fresh `scope_id` — aliasing is
type-equivalent, so a slot typed by the alias dispatches to the same overload as
a slot typed by the original. Struct / union / module / Result aliases route
through `register_type` (type-only); only the SIG-alias arm keeps
`register_nominal`. Anonymous `UNION (...)` is not a valid surface — every tagged
value carries a real per-declaration identity.

## Cycle close for mutually recursive nominals

Mutually recursive STRUCT / named-UNION pairs resolve through the
[`Bindings.pending_types`](../../src/machine/core/bindings.rs)
registry: STRUCT / named-UNION `body()` installs a
`PendingTypeEntry { kind, scope_id, schema_expr, edges }` before launching its
elaborator. The elaborator's `Resolution::Placeholder` arm records edges and
runs DFS from `current_decl_name`; a closed cycle invokes
[`close_type_cycle`](../../src/machine/model/types/resolver.rs), which
synchronously installs every member's **payload-empty** identity into
`bindings.types` via the panic-on-conflict
[`Scope::cycle_close_install_identity`](../../src/machine/core/scope.rs)
shim — enough to break the recursion, since identity equality ignores the
schema. Each member's eventual `finalize_struct` / `finalize_union` (or
Combine-finish for parked members) then routes through `register_type_upsert`:
the pre-installed entry compares `PartialEq`-equal to the schema-bearing
identity finalize built, so the upsert **overwrites it in place**, replacing the
empty payload with the real schema. The idempotent guard at each finalize site
short-circuits only when `types[name]` already holds a *populated* payload.
MODULE does not participate in `pending_types` (its body parks on the outer
scheduler's sibling dispatch deps, not on type-name resolution); its finalize
upserts directly.

## `NEWTYPE` and the `Wrapped` carrier

`NEWTYPE Distance = Number` declares a fresh nominal identity over a
transparent representation: declaration mints a per-declaration
[`KType::UserType { kind: UserTypeKind::Newtype { repr: Box<KType> }, scope_id, name }`](../../src/machine/model/types/ktype.rs)
and writes only `bindings.types`, the `repr` riding the identity as its payload —
the same type-only shape STRUCT / UNION / MODULE / Result use.

Construction (`Distance(3.0)`, `Bar(Foo(3.0))`) flows through
[`constructor_call`](../../src/machine/execute/dispatch/single_poll.rs)'s
`Newtype` arm — which branches on the resolved identity's `kind` — into
[`newtype_def::newtype_construct`](../../src/builtins/newtype_def.rs),
which schedules the value sub-expression via `add_dispatch` and waits on it
via a `Combine` whose finish closure type-checks against `repr` and produces
a
[`KObject::Wrapped { inner: NonWrappedRef<'a>, type_id: &'a KType }`](../../src/machine/model/values/kobject.rs)
carrier.

**Newtype-over-newtype collapse is encoded in the field type.**
[`NonWrappedRef`](../../src/machine/model/values/kobject.rs) is a
copy-newtype around `&'a KObject<'a>` whose sole constructor `peel` collapses
any `Wrapped` layer at construction time. `Bar(some_foo)` runs through
`NonWrappedRef::peel(some_foo)` and rewraps with `Bar`'s `type_id` — at most
one layer of wrapping at any point, and the invariant is a `cargo check`
guarantee rather than a caller-discipline contract.

The construction path is driven from the `constructor_call` fast lane (which
resolves the verb through `scope.resolve_type_with_chain` first and branches on
the resolved `kind`) rather than a registered builtin sharing the `[TypeExprRef,
…]` signature bucket — a sibling primitive on that bucket would re-dispatch
infinitely.

ATTR over a `KObject::Wrapped` falls through to `inner` via
[`access_field`'s `Wrapped` arm](../../src/builtins/attr.rs): an ATTR
overload typed `AnyUserType { kind: Newtype { repr: Box::new(Any) } }` reuses
`body_struct` because the lhs-shape dispatch lives inside `access_field`; the
recursion descends exactly one level by the collapse invariant. The ATTR
overload's slot is disjoint from the Struct / Module slots (the manual
`UserTypeKind::PartialEq` discriminates by kind), so dispatch picks without a
specificity tiebreaker. Missing-field diagnostics name the inner struct
(`b: Boxed = Point; b.z` reports `struct Point has no field z`) — the
fall-through is transparent at the diagnostic level too. The wildcard surface
name `Newtype` is intentionally *not* registered in
[`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs) —
it's reserved as the writable form once a builtin signature surfaces the need,
and otherwise appears only synthesized inside ATTR's
`AnyUserType { kind: Newtype { repr: Any } }` slot.
