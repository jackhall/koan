# User-declared nominal types

A `NEWTYPE` or named `UNION` declaration is a *nominal* type: its
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

A user `UNION` is not its own member family: it seals one `NewType` member per
variant and binds the union name to the anonymous
[`KType::Union`](../../src/machine/model/types/ktype.rs) of those members' `SetRef`s
— see [Unions dissolve into per-variant newtypes](#unions-dissolve-into-per-variant-newtypes)
below.

A member's nominal family is one of
[`KKind::{Newtype, TypeConstructor}`](../../src/machine/model/types/kkind.rs)
— the two families sitting strictly below `Proper` in the kind lattice
(`Any > {Module, Signature, Proper > {Newtype, TypeConstructor}}`). The family
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
`scope_id` are diagnostics, never identity. Two distinct nominals sit in distinct
sets (or distinct indices within one set), so they carry distinct identities —
the per-declaration-distinctness dispatch keys on. Because the identity is a
pointer, identity comparison never descends the (possibly cyclic) schema.

## Lift travels the set as one unit

Lifting any `SetRef` out of a dying frame is `Rc::clone` of the whole set
([lift.rs](../../src/machine/execute/lift.rs)). The recursive group travels as a
single unit, inherently cycle-aware — no copy, no visited-map traversal, no
`Rc<CallFrame>` anchor, because the set is `Rc`-owned rather than region-owned.
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

The [`KObject::Tagged`](../../src/machine/model/values/kobject.rs) carrier
carries `(set, index)` directly, populated at finalize time — it now backs only the
`TypeConstructor` family (`Result`, and the `CATCH` / `TRY` error machinery). `ktype()`
synthesizes `KType::SetRef { set, index }` by `Rc::clone`ing the carried set — the
dispatch identity is the set pointer and index, not the schema. Newtype instances —
scalar, record-repr, *and every user-`UNION` variant* — instead ride
[`KObject::Wrapped`](../../src/machine/model/values/kobject.rs), which carries a
`type_id: &KType` (the member `SetRef`). Module and
signature values ride the value channel's `Type` arm as
[`KType::Module { .. }`](../../src/machine/model/types/ktype.rs) /
`KType::Signature { .. }` ([`Carried::Type`](../../src/machine/model/values/carried.rs)); the
identity is the carried `&KType` itself rather than a synthesized shadow.

`bindings.data` holds only runtime instances. A value-position reference to a
nominal type token (passing `Outcome` to a constructor or ATTR call) surfaces the
[`bindings.types` identity in the `Type` arm](../../src/machine/execute/dispatch/resolve_type_identifier.rs)
on demand via `Scope::resolve_type_identifier` — no
value-side schema carrier exists for newtype / union / module / Result.

## Unions dissolve into per-variant newtypes

A user `UNION` has no nominal family of its own: it is the anonymous-union join of
one `NewType` per variant — the sum-side counterpart of the struct → record-repr
`NEWTYPE` collapse.
`UNION Maybe = (Some :Number None :Null)` seals a `RecursiveSet` of two
`KKind::NewType` members (`Some` over `Number`, `None` over `Null`) and binds `Maybe`
to [`KType::union_of([SetRef Some, SetRef None])`](../../src/machine/model/types/ktype_resolution.rs)
([union.rs](../../src/builtins/union.rs)). Variant tags are capitalized
[`Type` tokens](tokens.md): the `UNION` schema field-list runs under
[`FieldNameKind::Type`](../../src/parse/triple_list.rs), so a lowercase tag is a
parse error — a variant name is a nominal type, not a field.

**A variant value is an ordinary `KObject::Wrapped`** over its member `SetRef`, so its
`ktype()` reports that `SetRef` — there is no distinct tagged-value carrier and no
variant `KType`. A slot typed `:(Maybe Some)` is that member `SetRef`; a `:Maybe` slot
is the anonymous union, which admits any member via
[`KType::matches_value`](../../src/machine/model/types/ktype_predicates.rs)'s
per-member delegation. Each member `SetRef` is strictly more specific than the union
(each member is a subtype of the union), so a variant-typed overload wins over a
union-typed sibling that admits the same value.

**The variant-reference surface is the union-qualified sigil `:(Maybe Some)`** —
a variant reached through its union, with no global `:Some` name and no `.`
path operator. The dispatcher's union constructor arm
([apply_callable.rs](../../src/machine/execute/dispatch/apply_callable.rs))
disambiguates by body shape: a bare `Type`-token body (`Maybe Some`) yields the
member `SetRef` type value, while a payload body (`Maybe (Some 42)`) newtype-constructs
that member. An unknown variant name in either form is a schema error listing the
union's members. The member `SetRef` renders as its member name (`Some`), so
`:(Maybe Some)` round-trips.

**Nesting survives** because the wrap chooses between two payload dispositions
([`WrappedPayload`](../../src/machine/model/values/kobject.rs)): a transparent re-tag
(a `NEWTYPE` over a value already of that exact repr) *peels* one wrapper layer so
identities never stack, while a genuine variant construction *holds* the payload
verbatim — so a recursive union variant nesting another variant (`Succ (Zero null)`)
keeps every layer. This relaxes the older single-layer newtype-collapse invariant: the
`Wrapped` payload is no longer statically guaranteed non-`Wrapped`.

**`MATCH` selects by type** ([match_case.rs](../../src/builtins/match_case.rs) via
[`find_branch_body_by_type`](../../src/builtins/branch_walk.rs)). A member-name head
over a variant value admits by member `SetRef` identity and binds the wrapped payload
to `it`; a general type head resolves through the scope and binds the scrutinee
unchanged; the strictly most-specific admitting arm wins, and two arms with no strict
winner are an ambiguity error (ruling F1/F3). See
[unions and match-by-type](type-language-via-dispatch.md#anonymous-union-sigil).

The `TypeConstructor` carve-out: a `Result` value (`KKind::TypeConstructor`)
keeps the bare/applied ctor identity (`SetRef` / `ConstructorApply`) on its
`KObject::Tagged` carrier, so the `Result` / `CATCH` / `TRY` error machinery is
untouched, and `TRY` still selects arms by error-tag string
([try_with.rs](../../src/builtins/try_with.rs) via `find_branch_body_by_tag`). See
[error-handling.md](../error-handling.md).

## Type-only nominal install

NEWTYPE / UNION-named / MODULE / Result finalize write **only** `bindings.types`:
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
[`NominalSchema`](../../src/machine/model/types/recursive_set.rs) (`NewType` repr — one
per variant for a `UNION` — or a `TypeConstructor` schema + param names)
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
[elaboration.md](elaboration.md)): in a bare `NEWTYPE A = :{b :B}` / `NEWTYPE B = :{a :A}`
pair, whichever is written first forward-references the other, a position
error.

`RECURSIVE TYPES` co-declares the group as one shared set:

```
RECURSIVE TYPES Pair = (
    NEWTYPE A = :{b :B}
    NEWTYPE B = :{a :A}
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
- on the block's dep-finish, guarantees resolution — every member must have
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
and writes only `bindings.types` — the same type-only shape NEWTYPE / UNION / MODULE
/ Result use. The `repr` is not part of identity. A record repr
(`NEWTYPE Point = :{x :Number, y :Number}`) is a `NominalSchema::Newtype` over a
`KType::Record` — the product-side nominal form; `.x` reads the field through ATTR's
`Wrapped` fall-through over the record repr.

The [`NEWTYPE`](../../src/builtins/newtype_def.rs) declarator carries three overloads
selected by the repr part-kind:

- A **scalar / bare-leaf** repr (`= Number`, `= Foo`) rides the `OfKind(ProperType)` slot
  and resolves eagerly to a `KType`, sealing a plain singleton Newtype over it.
- A **non-record sigil** repr (`= :(LIST OF Elem)`) rides a `:SigiledTypeExpr` slot that
  captures the sigil *raw* — more specific than `OfKind(ProperType)`, so it wins with no
  admission-rule change. There is no self-reference to thread, so the shared `body`
  sub-dispatches the captured sigil to a resolved `KType` and seals a plain Newtype
  over it.
- A **record** repr (`= :{…}`) rides a distinct `:RecordType` slot — the sibling of
  `:SigiledTypeExpr`, also more specific than `OfKind(ProperType)` — routed to its own
  `body_record_repr` overload. Capturing the field list raw lets the declarator own its
  elaboration: it threads the binder name
  ([`Elaborator::with_threaded`](../../src/machine/model/types/resolver.rs)) through
  [`parse_typed_field_list_via_elaborator`](../../src/machine/model/types/typed_field_list.rs),
  so a self-reference (`NEWTYPE Node = :{value :Number, next :Node}`) lowers to a
  transient `RecursiveRef` and seals — via
  [`seal_recursive_refs`](../../src/machine/model/types/recursive_set.rs) (a `UNION` uses
  the sibling [`seal_union_refs`](../../src/machine/model/types/recursive_set.rs), which
  additionally maps the union's own name to the join of its variant members, ruling F2)
  — to a `SetLocal` back-edge into the declaring member's set. A `:(LIST OF Self)`
  field threads the same way, sealing `List(SetLocal)`, and a nested record field type
  (`:{inner :{owner :Node}}`) elaborates inline through the same walker so it threads
  too. A `NEWTYPE` member of a `RECURSIVE TYPES` block routes through this path,
  filling the block's shared set rather than minting a singleton.

Construction (`Distance(3.0)`, `Bar(Foo(3.0))`) flows through
[`type_call`](../../src/machine/execute/dispatch/single_poll.rs)'s `Newtype` arm —
which branches on the resolved member's `kind` — into
[`newtype_def::newtype_construct`](../../src/builtins/newtype_def.rs), which
schedules the value sub-expression via `dispatch_in_scope` and waits on it via a
dep-finish whose finish closure type-checks against `repr` and produces a
[`KObject::Wrapped { inner: WrappedPayload<'a>, type_id: &'a KType }`](../../src/machine/model/values/kobject.rs)
carrier.

**The wrap chooses peel-or-hold by the payload's identity.**
[`WrappedPayload`](../../src/machine/model/values/kobject.rs) is a copy-newtype
around `Rc<KObject<'a>>` with two constructors that record the wrapper's intent.
A **re-tag** — the constructed value's identity is exactly this repr, e.g.
`Bar(some_foo)` where `some_foo` is already a `Foo` and `NEWTYPE Bar = Foo` — takes
`WrappedPayload::peel`, collapsing one `Wrapped` layer so identities never stack.
A **genuine construction** — the payload is a *member* of the type being built, whose
identity differs from the repr, e.g. a `UNION` variant `Succ :Nat` wrapping another
`Nat` variant — takes `WrappedPayload::hold`, preserving the payload verbatim so the
recursion the dissolved-union model needs survives (`Succ (Zero null)` keeps both
layers). `check_newtype_repr` decides which by comparing the payload's `ktype()` to the
projected `repr` before the witness build. The choice replaces the older single-layer
collapse invariant, which peeled unconditionally.

The construction path is driven from the `type_call` fast lane (which resolves the
verb through `scope.resolve_type_with_chain` first and branches on the resolved
member's `kind`) rather than a registered builtin sharing the `[OfKind(ProperType), …]`
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
value errors "a value with fields"), descending one level per access.
Specificity (`Any` ≺ `OfKind` ≺ `Identifier`) keeps this unambiguous with the
sibling overloads: an `Identifier` lhs wins `body_identifier`, a module / type-token lhs
wins its `OfKind` overload, and only a bare runtime value falls through here. Missing-field
diagnostics name the inner record (`b: Boxed = Point; b.z` reports the field miss on
`Point`) — the fall-through is transparent at the diagnostic level too. The nominal-family
keyword `Newtype` is *not* registered in
[`KType::from_name`](../../src/machine/model/types/ktype_resolution.rs); the `OfKind(Newtype)`
slot is type-channel-only and never matches a runtime value.
