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
- [`KType::Variant { set: Rc<RecursiveSet>, index, tag }`](../../src/machine/model/types/ktype.rs)
  is a **refinement** of a `Tagged`-kind member: `(set, index)` names the union,
  `tag` selects one variant within it. It is what a user-`UNION` value's `ktype()`
  reports, and what a `:(Maybe Some)` slot carries — see
  [Tagged-union variants](#tagged-union-variants) below.

A member's nominal family is one of
[`KKind::{Tagged, Newtype, TypeConstructor}`](../../src/machine/model/types/kkind.rs)
— the three families sitting strictly below `Proper` in the kind lattice
(`Any > {Module, Signature, Proper > {Tagged, Newtype, TypeConstructor}}`). The family
is stored on the set member (`set.member(index).kind`), payload-free and `Copy`, with a
`surface_keyword()` accessor. A slot that wants "any user-declared type of family X" is an
[`KType::OfKind(KKind)`](../../src/machine/model/types/ktype.rs) carrying that family;
because `OfKind` is **type-channel-only** it admits the *type value* of the family,
classified by `kind_of`, never a runtime instance. The nominal-family keywords are pinned
for diagnostic rendering only and are not registered as writable surface names (no entry
in [`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs)).

Modules and signatures live in their own KType variants —
[`KType::Module { module, frame }`](../../src/machine/model/types/ktype.rs)
for first-class module values,
[`KType::Signature { sig, pinned_slots }`](../../src/machine/model/types/ktype.rs)
for first-class signature values (and for the satisfies-this-signature slot
constraint — one variant, disambiguated by position), and
[`KType::AbstractType { source, name }`](../../src/machine/model/types/ktype.rs)
for abstract-type members (SIG-declared or minted by opaque ascription) — with
`KType::OfKind(KKind::Module)` and `KType::OfKind(KKind::Signature)` as the matching wildcards
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
place a concrete `SetRef` strictly below `OfKind(K)` of its own family, `OfKind(K)`
strictly below `OfKind(Proper)`, and that below `Any` in `is_more_specific_than`. Because
`OfKind` is type-channel-only, an `OfKind(K)` slot ranks against *type values* by
`kind_of` subsumption (`KKind::admits` / `KKind::strictly_below`), reading the member's
kind via `set.member(index).kind` — so each family ranks alongside the others with no
per-kind branching at the dispatcher. The module/signature
variants follow the parallel stratification: `KType::Module { .. }` ≺
`KType::OfKind(KKind::Module)` ≺ `Any`, and `KType::Signature { .. }` ≺
`KType::OfKind(KKind::Signature)` ≺ `Any`. This is the identity-and-wildcard slice of Layer 3 of the
[lookup → admit protocol](lookup-protocol.md); the predicate is the same one
every dispatch admit pass runs.

## Value carriers and the type / value partition

Instance carriers — [`KObject::Struct`](../../src/machine/model/values/kobject.rs)
and [`KObject::Tagged`](../../src/machine/model/values/kobject.rs) — carry
`(set, index)` directly, populated at finalize time. `ktype()` on either
synthesizes `KType::SetRef { set, index }` by `Rc::clone`ing the carried set — the
dispatch identity is the set pointer and index, not the schema. Module and
signature values ride the value channel's `Type` arm as
[`KType::Module { .. }`](../../src/machine/model/types/ktype.rs) /
`KType::Signature { .. }` ([`Carried::Type`](../../src/machine/model/values/carried.rs)); the
identity is the carried `&KType` itself rather than a synthesized shadow.

`bindings.data` holds only runtime instances. A value-position reference to a
nominal type token (passing `Outcome` to a constructor or ATTR call) surfaces the
[`bindings.types` identity in the `Type` arm](../../src/machine/execute/dispatch/resolve_type_expr.rs)
on demand via `resolve_type_leaf_carrier` — no
value-side schema carrier exists for struct / union / module / Result.

## Tagged-union variants

A declared `UNION` variant is its own dispatchable type. A user-`UNION`
(`KKind::Tagged`) value's `ktype()` reports
[`KType::Variant { set, index, tag }`](../../src/machine/model/types/ktype.rs) —
a refinement of the union member at `(set, index)` selecting the inhabited `tag` —
rather than the bare `SetRef`
([kobject.rs](../../src/machine/model/values/kobject.rs)). Variant tags are
capitalized [`Type` tokens](tokens.md): `UNION Maybe = (Some :Number None :Null)`,
not `some` / `none`. The `UNION` schema field-list runs under
[`FieldNameKind::Type`](../../src/parse/triple_list.rs), so a lowercase tag is a
parse error — the tokenizer keys `Type` vs `Identifier` purely on capitalization,
and a variant has to be type-classified to flow through dispatch.

**Identity is `(set ptr, index, tag)`** — manual `PartialEq` / `Hash` arms keyed
on the set pointer, member index, and tag string, never the (cyclic) schema
([ktype.rs](../../src/machine/model/types/ktype.rs)). Two same-payload variants
of one union stay distinct because the tag is part of identity, and a variant of
union A never equals a variant of union B. The whole set rides every `Variant`,
so lifting one is `Rc::clone` of the group, exactly as for `SetRef`.

**Variants slot into the specificity stratification** below their union: a
concrete `Variant` ≺ its union's `SetRef` ≺ `OfKind(Tagged)` ≺
`Any` ([ktype_predicates.rs](../../src/machine/model/types/ktype_predicates.rs)).
So a slot typed `:(Maybe Some)` admits only `Some` values and a `:Maybe` slot admits
*any* variant (the union `SetRef` arm explicitly matches any `Tagged`-kind value
of that union) — and a variant-typed overload wins over a union-typed sibling that also
admits the value. The `OfKind(Tagged)` family kind is type-channel-only, so it admits a
Tagged *type value* by `kind_of`, never a runtime instance.

**The variant-reference surface is the union-qualified sigil `:(Maybe Some)`** —
a variant type reached through its union, with no global `:Some` name and no `.`
path operator. The dispatcher's `Tagged` constructor arm
([apply_callable.rs](../../src/machine/execute/dispatch/apply_callable.rs))
disambiguates by body shape: a bare `Type`-token body (`Maybe Some`) yields the
variant `KType` value, while a payload body (`Maybe (Some 42)`) constructs. An
unknown tag at the reference surface is a schema error listing the union's
variants. The variant renders back to `:(Maybe Some)` so it round-trips.

The `TypeConstructor` carve-out: a `Result` value (`KKind::TypeConstructor`)
keeps the bare/applied union identity (`SetRef` / `ConstructorApply`), so the
`Result` / `CATCH` / `TRY` error machinery is untouched — only `Tagged`-kind
values report a `Variant`. See [error-handling.md](../error-handling.md). Routing
`MATCH` itself through ordinary type-dispatch (instead of its current distinct
fast-track form) and recursive variant references inside a schema are open work,
tracked under
[tagged-union variants as dispatchable types](../../roadmap/type_language/tagged-variant-types.md).

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
(`KType::Signature { .. }` in the `Type` arm) it is the identity-bearing signature
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
/ Result use. The `repr` is not part of identity. A record repr
(`NEWTYPE Point = :{x :Number, y :Number}`) is a `NominalSchema::Newtype` over a
`KType::Record` — the product-side nominal form; `.x` reads the field through ATTR's
`Wrapped` fall-through over the record repr.

The [`NEWTYPE`](../../src/builtins/newtype_def.rs) declarator carries three overloads
selected by the repr part-kind:

- A **scalar / bare-leaf** repr (`= Number`, `= Foo`) rides the `:TypeExprRef` slot
  and resolves eagerly to a `KType`, sealing a plain singleton Newtype over it.
- A **non-record sigil** repr (`= :(LIST OF T)`) rides a `:SigiledTypeExpr` slot that
  captures the sigil *raw* — more specific than `:TypeExprRef`, so it wins with no
  admission-rule change. There is no self-reference to thread, so the shared `body`
  sub-dispatches the captured sigil to a resolved `KType` and seals a plain Newtype
  over it.
- A **record** repr (`= :{…}`) rides a distinct `:RecordType` slot — the sibling of
  `:SigiledTypeExpr`, also more specific than `:TypeExprRef` — routed to its own
  `body_record_repr` overload. Capturing the field list raw lets the declarator own its
  elaboration: it threads the binder name
  ([`Elaborator::with_threaded`](../../src/machine/model/types/resolver.rs)) through
  [`parse_typed_field_list_via_elaborator`](../../src/machine/model/types/typed_field_list.rs),
  so a self-reference (`NEWTYPE Node = :{value :Number, next :Node}`) lowers to a
  transient `RecursiveRef` and seals — via the shared
  [`finalize_nominal_member`](../../src/machine/model/types/recursive_set.rs) /
  [`seal_recursive_refs`](../../src/machine/model/types/recursive_set.rs) path `UNION`
  uses — to a `SetLocal` back-edge into the declaring member's set. A `:(LIST OF Self)`
  field threads the same way, sealing `List(SetLocal)`, and a nested record field type
  (`:{inner :{owner :Node}}`) elaborates inline through the same walker so it threads
  too. A `NEWTYPE` member of a `RECURSIVE TYPES` block routes through this path,
  filling the block's shared set rather than minting a singleton.

Construction (`Distance(3.0)`, `Bar(Foo(3.0))`) flows through
[`type_call`](../../src/machine/execute/dispatch/single_poll.rs)'s `Newtype` arm —
which branches on the resolved member's `kind` — into
[`newtype_def::newtype_construct`](../../src/builtins/newtype_def.rs), which
schedules the value sub-expression via `dispatch_in_scope` and waits on it via a
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
[`access_field`'s `Wrapped` arm](../../src/builtins/attr.rs). A runtime `Wrapped` lhs is
matched by a *type*, never by a kind: it lands in the least-specific `s: Any` ATTR
overload, and `access_field` validates the `Wrapped` shape in the body (a non-`Wrapped`
value errors "a value with fields"), descending exactly one level by the collapse
invariant. Specificity (`Any` ≺ `OfKind` ≺ `Identifier`) keeps this unambiguous with the
sibling overloads: an `Identifier` lhs wins `body_identifier`, a module / type-token lhs
wins its `OfKind` overload, and only a bare runtime value falls through here. Missing-field
diagnostics name the inner record (`b: Boxed = Point; b.z` reports the field miss on
`Point`) — the fall-through is transparent at the diagnostic level too. The nominal-family
keyword `Newtype` is *not* registered in
[`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs); the `OfKind(Newtype)`
slot is type-channel-only and never matches a runtime value.
