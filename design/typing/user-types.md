# User-declared nominal types

A `STRUCT`, named `UNION`, or `NEWTYPE` declaration is a *nominal* type: its
identity is its declaration, not its shape. Nominal identity lives in the
[`RecursiveSet`](../../src/machine/model/types/recursive_set.rs) — one
`Rc`-owned strongly-connected component of mutually-recursive nominals. A
non-recursive type is a *singleton* set of one member; a self-recursive type, an
`A ↔ B` pair, or a longer cycle is one set of several members.

Three `KType` variants reference set members:

- [`KType::SetRef { set: Rc<RecursiveSet>, index }`](../../src/machine/model/types/ktype.rs)
  is the **external** handle — what `bindings.types` holds, what a non-member's
  field type names, what a parameter slot carries, what a constructed value's
  `ktype()` reports. It carries the whole `Rc<RecursiveSet>` plus the member
  index.
- [`KType::SetLocal(index)`](../../src/machine/model/types/ktype.rs) is the
  **intra-set sibling** reference — a bare index resolved against the ambient set
  during deep traversal only. It carries no `Rc`, so a set holds no internal
  refcount cycle and frees once its last external handle drops (load-bearing: an
  internal `SetRef` would pin the set's own allocation and leak the whole group).
- [`KType::RecursiveGroup(Rc<RecursiveSet>)`](../../src/machine/model/types/ktype.rs)
  is the first-class handle to a whole set, bound by a `RECURSIVE TYPES` group
  name. It is inert in value dispatch — it names a group of types, not a value
  type — and is reserved for value-language cycle construction.

The companion
[`KType::AnyUserType { kind: NominalKind }`](../../src/machine/model/types/ktype.rs)
wildcard accepts any nominal carrier of the matching kind. The surface family is
[`enum NominalKind { Struct, Tagged, Newtype, TypeConstructor }`](../../src/machine/model/types/recursive_set.rs)
— payload-free and `Copy`, with a `surface_keyword()` accessor. The names
`"Struct"` / `"Tagged"` lower to `AnyUserType { kind }` in
[`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs);
[`scope.register_type`](../../src/builtins.rs) agrees so the type-resolver and the
builtin registry produce the same wildcard carrier.

Modules and signatures live in their own KType variants —
[`KType::Module { module, frame }`](../../src/machine/model/types/ktype.rs)
for first-class module values,
[`KType::Signature { sig, pinned_slots }`](../../src/machine/model/types/ktype.rs)
for first-class signature values (and for the satisfies-this-signature slot
constraint — one variant, disambiguated by position), and
[`KType::AbstractType { source, name }`](../../src/machine/model/types/ktype.rs)
for abstract-type members (SIG-declared or minted by opaque ascription) — with
`KType::AnyModule` and `KType::AnySignature` as the matching wildcards
(see [modules.md](modules.md) for the carrier model).

## Identity is the set pointer plus index

A member's identity is `(Rc::as_ptr(set), index)` — never its schema, which may
be cyclic. `SetRef` equality and hashing key on the pointer and index only
([ktype.rs](../../src/machine/model/types/ktype.rs)); the member's `name` and
`scope_id` are diagnostics, never identity. Two distinct STRUCTs sit in distinct
sets (or distinct indices within one set), so they carry distinct identities —
the per-declaration-distinctness dispatch keys on. Because the identity is a
pointer, identity comparison never descends the (possibly cyclic) schema.

## Lift travels the set as one unit

Lifting any `SetRef` out of a dying frame is `Rc::clone` of the whole set
([lift.rs](../../src/machine/execute/lift.rs)). The recursive group travels as a
single unit, inherently cycle-aware — no copy, no visited-map traversal, no
`Rc<CallArena>` anchor, because the set is `Rc`-owned rather than arena-owned.
The `SetLocal` siblings inside a member's schema ride along unchanged; they
resolve against the cloned set's pointer.

## Specificity stratification

Predicate arms
([`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs))
place a concrete `SetRef` strictly below `AnyUserType { kind: K }` strictly below
`Any` in `is_more_specific_than`, and `AnyUserType { kind }` matches any
`KObject::Struct` / `Tagged` whose member `kind` matches. The check reads the
member's kind via `set.member(index).kind`, so each kind ranks alongside the
others with no per-kind branching at the dispatcher. The module/signature
variants follow the parallel stratification: `KType::Module { .. }` ≺
`KType::AnyModule` ≺ `Any`, and `KType::Signature { .. }` ≺ `KType::AnySignature`
≺ `Any`. This is the identity-and-wildcard slice of Layer 3 of the
[lookup → admit protocol](lookup-protocol.md); the predicate is the same one
every dispatch admit pass runs.

## Value carriers and the type / value partition

Instance carriers — [`KObject::Struct`](../../src/machine/model/values/kobject.rs)
and [`KObject::Tagged`](../../src/machine/model/values/kobject.rs) — carry
`(set, index)` directly, populated at finalize time. `ktype()` on either
synthesizes `KType::SetRef { set, index }` by `Rc::clone`ing the carried set — the
dispatch identity is the set pointer and index, not the schema. Module and
signature values ride
[`KObject::KTypeValue(KType::Module { .. })`](../../src/machine/model/values/kobject.rs) /
`KObject::KTypeValue(KType::Signature { .. })`; `ktype()` projects the carried
`KType` directly, so the identity is the carrier rather than a synthesized shadow.

`bindings.data` holds only runtime instances. A value-position reference to a
nominal type token (passing `Outcome` to a constructor or ATTR call) synthesizes
[`KObject::KTypeValue(identity)`](../../src/machine/execute/dispatch/resolve_type_expr.rs)
on demand from the `bindings.types` entry via `coerce_type_token_value` — no
value-side schema carrier exists for struct / union / module / Result.

## Type-only nominal install

STRUCT / UNION-named / MODULE / Result finalize write **only** `bindings.types`:
each builds its identity (a `KType::SetRef` into its sealed set, or
`KType::Module { module, frame }`) and installs it through
[`Scope::register_type_upsert`](../../src/machine/core/scope.rs), which inserts if
absent and overwrites a `PartialEq`-equal entry, surfacing `Rebind` on a genuine
non-equal collision. The schema rides inside the set member, so construction reads
fields / variant types straight off the projected member; there is no
second-namespace write to keep in sync. The single-home invariant —
Type-classed name lookups go through `Scope::resolve_type` only — holds because
the identity *is* the only entry.

SIG installs the same way, through
[`Scope::register_type_upsert`](../../src/machine/core/scope.rs): a single
`KType::Signature { sig, pinned_slots }` identity in `bindings.types` serves
*both* roles. As a slot annotation (`Er :OrderedSig`) it is the constraint form —
"any module satisfying OrderedSig"; as a value
(`KTypeValue(KType::Signature { .. })`) it is the identity-bearing signature
carrier, carrying the live `decl_scope` via `sig`. The roles are disambiguated by
position, not by separate variants, so no value-side carrier is written;
`bindings.data` holds zero type carriers. Every nominal binder is a single
type-namespace write.

`LET <Type-class> = <module/sig/struct-value>` (e.g.
`LET IntOrdA = (IntOrd :| OrderedSig)`) installs the *original* type's identity
under the alias name rather than minting a fresh set — aliasing is
type-equivalent, so a slot typed by the alias dispatches to the same overload as
a slot typed by the original. Struct / union / module / Result / signature aliases
all route through `register_type` (type-only). Anonymous `UNION (...)` is not a
valid surface — every tagged value carries a real per-declaration identity.

## Schemas: members fill their slot at finalize

Each set member carries its schema in a two-phase
[`RefCell`](../../src/machine/model/types/recursive_set.rs) cell: the set is sealed
with its membership and each member's `kind` known eagerly, but the
[`NominalSchema`](../../src/machine/model/types/recursive_set.rs) (`Struct` record,
`Tagged` tag→type map, `Newtype` repr, or `TypeConstructor` schema + param names)
is filled at the member's own finalize. A member's schema names its siblings as
`SetLocal` indices.

Two deep walks convert between the internal and external reference forms
([recursive_set.rs](../../src/machine/model/types/recursive_set.rs)):

- `seal_recursive_refs` runs at finalize, sealing a member's schema into the set:
  a transient `RecursiveRef(name)` whose name is a set member becomes
  `SetLocal(index)`, and a `SetRef` that resolved back into the same set (a
  cross-sibling reference that hit the seal's pre-installed `SetRef`) folds to
  `SetLocal(index)` — leaving that internal `SetRef` would hold an `Rc` to the
  set's own allocation, the refcount cycle that leaks.
- `resolve_set_locals` runs the other direction when projecting a member's schema
  for construction / navigation / matching, replacing each `SetLocal(i)` with an
  external `SetRef { set, i }`. `RecursiveSet::projected_schema` produces the
  navigable [`ProjectedSchema`](../../src/machine/model/types/recursive_set.rs)
  for a member.

## `RECURSIVE TYPES` — the mutual-recursion construct

A self-recursive type needs no special construct: the binder threads its own name,
so a back-edge (a field naming the declaring type) lowers to a transient
`RecursiveRef` and seals to a `SetLocal` against the declaring member's own
singleton set. Mutual recursion of two or more types *does* need a construct,
because type names obey strict source order (see
[elaboration.md](elaboration.md)): in a bare `STRUCT A = (b :B)` / `STRUCT B =
(a :A)` pair, whichever is written first forward-references the other, a position
error.

`RECURSIVE TYPES` co-declares the group as one shared set:

```
RECURSIVE TYPES Pair = (
    STRUCT A = (b :B)
    STRUCT B = (a :A)
)
```

The builtin ([`recursive_types.rs`](../../src/builtins/recursive_types.rs)):

- discovers each member's `(name, kind)` from the body declarations, mints one
  shared `RecursiveSet` with the members `pending`, and dispatches the
  declarations against a child scope carrying the set
  ([`Scope::child_recursive_group`](../../src/machine/core/scope.rs) /
  [`nearest_recursive_set`](../../src/machine/core/scope.rs));
- pre-installs each member's external `SetRef` into the child so a member's own
  finalize fills the shared set rather than minting a singleton. Inside the
  block every member name is threaded, so a cross-reference lowers to a transient
  `RecursiveRef` and seals to a `SetLocal` index into the shared set;
- on the block's Combine finish, guarantees resolution — every member must have
  sealed, or a forward reference named a name outside the group, raised as a
  localized shape error at the block boundary. The sealed members mirror into the
  enclosing scope as external `SetRef` handles (`A`, `B` bind as ordinary type
  names), and the group name (`Pair`) binds a `KType::RecursiveGroup` handle.

`RECURSIVE TYPES` is the only way to express mutual recursion of two or more
types; the block scopes its threaded group within strict lexical order, so a
forward reference is either discharged into the set or a localized error, never a
placeholder that survives into a sealed type.

## `NEWTYPE` and the `Wrapped` carrier

`NEWTYPE Distance = Number` declares a fresh nominal identity over a transparent
representation. Declaration seals a singleton set whose one member is a
[`NominalSchema::Newtype { repr }`](../../src/machine/model/types/recursive_set.rs)
and writes only `bindings.types` — the same type-only shape STRUCT / UNION / MODULE
/ Result use. The `repr` is not part of identity.

Construction (`Distance(3.0)`, `Bar(Foo(3.0))`) flows through
[`type_call`](../../src/machine/execute/dispatch/single_poll.rs)'s `Newtype` arm —
which branches on the resolved member's `kind` — into
[`newtype_def::newtype_construct`](../../src/builtins/newtype_def.rs), which
schedules the value sub-expression via `add_dispatch` and waits on it via a
`Combine` whose finish closure type-checks against `repr` and produces a
[`KObject::Wrapped { inner: NonWrappedRef<'a>, type_id: &'a KType }`](../../src/machine/model/values/kobject.rs)
carrier.

**Newtype-over-newtype collapse is encoded in the field type.**
[`NonWrappedRef`](../../src/machine/model/values/kobject.rs) is a copy-newtype
around `&'a KObject<'a>` whose sole constructor `peel` collapses any `Wrapped`
layer at construction time. `Bar(some_foo)` runs through
`NonWrappedRef::peel(some_foo)` and rewraps with `Bar`'s `type_id` — at most one
layer of wrapping at any point, and the invariant is a `cargo check` guarantee
rather than a caller-discipline contract.

The construction path is driven from the `type_call` fast lane (which resolves the
verb through `scope.resolve_type_with_chain` first and branches on the resolved
member's `kind`) rather than a registered builtin sharing the `[TypeExprRef, …]`
signature bucket — a sibling primitive on that bucket would re-dispatch infinitely.

The `Wrapped` carrier also backs **opaque VAL-slot re-tags**: an ATTR read of a
value-side slot from an opaquely-ascribed module re-wraps the value with the
per-call abstract identity the SIG names, so the read reports the abstract type
rather than its representation. The two uses share the variant — distinguished by
the `type_id`'s KType (a `SetRef` to a `Newtype`-kind member for construction, an
`AbstractType` for the slot re-tag) — and the same collapse and ATTR fall-through
rules apply to both. See
[modules.md § VAL-slot reads carry the abstract member identity](modules.md#val-slot-reads-carry-the-abstract-member-identity).

ATTR over a `KObject::Wrapped` falls through to `inner` via
[`access_field`'s `Wrapped` arm](../../src/builtins/attr.rs): an ATTR overload
typed `AnyUserType { kind: Newtype }` reuses `body_struct` because the lhs-shape
dispatch lives inside `access_field`; the recursion descends exactly one level by
the collapse invariant. The ATTR overload's slot is disjoint from the Struct /
Module slots (the `AnyUserType` wildcards discriminate by `kind`), so dispatch
picks without a specificity tiebreaker. Missing-field diagnostics name the inner
struct (`b: Boxed = Point; b.z` reports `struct Point has no field z`) — the
fall-through is transparent at the diagnostic level too. The wildcard surface name
`Newtype` is intentionally *not* registered in
[`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs) — it's
reserved as the writable form once a builtin signature surfaces the need, and
otherwise appears only synthesized inside ATTR's `AnyUserType { kind: Newtype }`
slot.
