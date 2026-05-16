# User-declared nominal types

[`KType::UserType { kind, scope_id, name }`](../../src/runtime/machine/model/types/ktype.rs)
is the per-declaration identity tag for every user-declared nominal type —
`STRUCT`, named `UNION`, `MODULE`, opaque-ascription module identities, and
`NEWTYPE`. The companion
[`KType::AnyUserType { kind }`](../../src/runtime/machine/model/types/ktype.rs)
wildcard accepts any `UserType` of the matching kind.

[`enum UserTypeKind { Struct, Tagged, Module, Newtype { repr }, TypeConstructor { param_names } }`](../../src/runtime/machine/model/types/ktype.rs)
has a `surface_keyword()` accessor; the two payload-carrying variants
(`Newtype`, `TypeConstructor`) have a manual `PartialEq` that ignores their
payloads so wildcard / identity comparisons key on kind and `(scope_id, name)`
only. The surface names `"Struct"` / `"Tagged"` / `"Module"` lower to
`AnyUserType { kind }` in
[`KType::from_name`](../../src/runtime/machine/model/types/ktype_resolution.rs),
and [`scope.register_type`](../../src/runtime/builtins.rs) agrees so the
type-resolver and the builtin registry produce the same wildcard carrier.

## Specificity stratification

Predicate arms
([`ktype_predicates.rs`](../../src/runtime/machine/model/types/ktype_predicates.rs))
place `UserType { kind: K, .. }` strictly below `AnyUserType { kind: K }`
strictly below `Any` in `is_more_specific_than`, and `AnyUserType { kind }`
matches any `KObject::Struct` / `Tagged` / `KModule` of the matching kind.
Each kind ranks alongside the others with no per-kind branching at the
dispatcher.

## Value carriers and dual-write

Value carriers — [`KObject::Struct`](../../src/runtime/machine/model/values/kobject.rs),
[`KObject::Tagged`](../../src/runtime/machine/model/values/kobject.rs),
[`KObject::StructType`](../../src/runtime/machine/model/values/kobject.rs),
[`KObject::TaggedUnionType`](../../src/runtime/machine/model/values/kobject.rs),
and `KObject::KModule` — carry `(scope_id, name)` identity fields populated
at finalize time via the `scope as *const _ as usize` scheme `Module::scope_id()`
uses. `ktype()` on a `KObject::Struct` / `Tagged` / `KModule` reconstructs
`KType::UserType { kind, scope_id, name }`, while schema carriers (`StructType`
/ `TaggedUnionType`) keep reporting `KType::Type` — they are values *of the
meta-type*, not user-typed values.

STRUCT / UNION-named / MODULE / SIG finalize each route through the
[`Scope::register_nominal`](../../src/runtime/machine/core/scope.rs) shim,
which transactionally writes `bindings.types[name] = &KType` and
`bindings.data[name] = &KObject` together so the single-home invariant —
Type-classed name lookups go through `Scope::resolve_type` only — holds. SIG
declarations write `KType::SignatureBound { sig_id, sig_path }` on the type
side.

`LET <Type-class> = <module/sig/struct-value>` (e.g.
`LET IntOrdA = (IntOrd :| OrderedSig)`) also dual-writes, preserving the
*original* carrier's identity rather than minting a fresh `scope_id` for the
alias name — aliasing is type-equivalent, so a slot typed by the alias
dispatches to the same overload as a slot typed by the original. Anonymous
`UNION (...)` is not a valid surface — every tagged value carries a real
per-declaration identity.

## Cycle close for mutually recursive nominals

Mutually recursive STRUCT / named-UNION pairs resolve through the
[`Bindings.pending_types`](../../src/runtime/machine/core/bindings.rs)
registry: STRUCT / named-UNION `body()` installs a
`PendingTypeEntry { kind, scope_id, schema_expr, edges }` before launching its
elaborator. The elaborator's `Resolution::Placeholder` arm records edges and
runs DFS from `current_decl_name`; a closed cycle invokes
[`close_type_cycle`](../../src/runtime/machine/model/types/resolver.rs), which
synchronously installs every member's identity into `bindings.types` via the
panic-on-conflict
[`Scope::cycle_close_install_identity`](../../src/runtime/machine/core/scope.rs)
shim. Each member's eventual `finalize_struct` / `finalize_union` (or
Combine-finish for parked members) then routes through `try_register_nominal`'s
cycle-close-idempotent arm — `types` is pre-populated with a matching identity,
so only the carrier writes to `data`. Defense-in-depth: every nominal finalize
site also short-circuits to the existing carrier when both `types[name]` and
`data[name]` are populated at entry. MODULE does not participate in
`pending_types` (its body parks on the outer scheduler's sibling dispatch deps,
not on type-name resolution); the idempotent guard still lives in MODULE
finalize for symmetry.

## `NEWTYPE` and the `Wrapped` carrier

`NEWTYPE Distance = Number` declares a fresh nominal identity over a
transparent representation: declaration mints a per-declaration
[`KType::UserType { kind: UserTypeKind::Newtype { repr: Box<KType> }, scope_id, name }`](../../src/runtime/machine/model/types/ktype.rs)
and writes only `bindings.types` — unlike STRUCT / UNION / MODULE, NEWTYPE has
no value-side schema carrier (no payload to bind at the declaration site).

Construction (`Distance(3.0)`, `Bar(Foo(3.0))`) flows through `type_call`'s
`Newtype` arm into
[`newtype_def::newtype_construct`](../../src/runtime/builtins/newtype_def.rs),
which schedules the value sub-expression via `add_dispatch` and waits on it
via a `Combine` whose finish closure type-checks against `repr` and produces
a
[`KObject::Wrapped { inner: NonWrappedRef<'a>, type_id: &'a KType }`](../../src/runtime/machine/model/values/kobject.rs)
carrier.

**Newtype-over-newtype collapse is encoded in the field type.**
[`NonWrappedRef`](../../src/runtime/machine/model/values/kobject.rs) is a
copy-newtype around `&'a KObject<'a>` whose sole constructor `peel` collapses
any `Wrapped` layer at construction time. `Bar(some_foo)` runs through
`NonWrappedRef::peel(some_foo)` and rewraps with `Bar`'s `type_id` — at most
one layer of wrapping at any point, and the invariant is a `cargo check`
guarantee rather than a caller-discipline contract.

The construction path is driven from `type_call::body` (which resolves the
verb through `scope.resolve_type` first and branches on the resolved `kind`)
rather than a second registered builtin: a sibling primitive would share
`type_call`'s `[TypeExprRef, …]` signature bucket and re-dispatch infinitely.

ATTR over a `KObject::Wrapped` falls through to `inner` via
[`access_field`'s `Wrapped` arm](../../src/runtime/builtins/attr.rs): an ATTR
overload typed `AnyUserType { kind: Newtype { repr: Box::new(Any) } }` reuses
`body_struct` because the lhs-shape dispatch lives inside `access_field`; the
recursion descends exactly one level by the collapse invariant. The ATTR
overload's slot is disjoint from the Struct / Module slots (the manual
`UserTypeKind::PartialEq` discriminates by kind), so dispatch picks without a
specificity tiebreaker. Missing-field diagnostics name the inner struct
(`b: Boxed = Point; b.z` reports `struct Point has no field z`) — the
fall-through is transparent at the diagnostic level too. The wildcard surface
name `Newtype` is intentionally *not* registered in
[`KType::from_name`](../../src/runtime/machine/model/types/ktype_resolution.rs) —
it's reserved as the writable form once a builtin signature surfaces the need,
and otherwise appears only synthesized inside ATTR's
`AnyUserType { kind: Newtype { repr: Any } }` slot.
