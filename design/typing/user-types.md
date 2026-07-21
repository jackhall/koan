# User-declared nominal types

A `NEWTYPE` or named `UNION` declaration is a *nominal* type: its
identity is its declaration, not its shape. Nominal identity is content-addressed
in the run-frame registry ([type-registry.md](type-registry.md)): each member is an
interned [`TypeNode::SetMember`](../../src/machine/model/types/node.rs) whose
identity unit is its own *strongly-connected component* under the sibling-reference
relation — not its declaration group. A non-recursive type is a singleton
component; a self-recursive type, an `A ↔ B` pair, or a longer cycle is one
component of several members.

Three node kinds carry the model:

- [`TypeNode::SetMember { scc_digest, index, scc_size, name, kind, schema }`](../../src/machine/model/types/node.rs)
  is one sealed member. Its `KType` handle is the `Copy` `(scc_digest, index)` folded
  into one digest — what `bindings.types` holds, what a non-member's field type
  names, what a parameter slot carries, what a constructed value's `ktype()` reports.
  A sibling reference inside its sealed `schema` is the sibling's own absolute member
  handle: a cyclic composition edge the insert-only registry holds without
  refcounting.
- [`TypeNode::Sibling(index)`](../../src/machine/model/types/node.rs) is the
  **relative** sibling reference used *only* inside a pre-seal group window — a bare
  index meaningful against the ambient window. It is ordinary interned content, but it
  never appears in a sealed schema, never reaches the predicates, and never rides a
  value; the seal rewrites each one to an absolute member handle.
- [`TypeNode::Group { members }`](../../src/machine/model/types/node.rs)
  is the first-class handle to a whole declared group, bound by a `RECURSIVE TYPES`
  group name. It is inert in value dispatch — it names a group of types, not a value
  type — and is reserved for value-language cycle construction.

A user `UNION` is not its own member family: it seals one `NewType` member per
variant and binds the union name to the anonymous
[`TypeNode::Union`](../../src/machine/model/types/node.rs) of those members' handles
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

Signatures live in their own KType variant —
[`KType::Signature { sig, pinned_slots }`](../../src/machine/model/types/ktype.rs)
for first-class signature values (and for the satisfies-this-signature slot
constraint — one variant, disambiguated by position) — with
`KType::OfKind(KKind::Signature)` as the matching wildcard, alongside
[`KType::AbstractType { source, name }`](../../src/machine/model/types/ktype.rs)
for abstract-type members (SIG-declared or minted by opaque ascription). A **module**
has no KType variant: it is a value, typed by its principal signature
(`Signature { sig: SelfOf(m), .. }`) — see [modules.md](modules.md) for the carrier
model.

## Identity is the SCC digest plus index

A member's identity is `(SCC digest, index)` — the content digest of the member's
own strongly-connected component plus its index in that component's canonical
(name-order) presentation. The digest is minted at seal from finished content, never
a live walk of the schema, which may be cyclic
([recursive_group_window.rs](../../src/machine/model/types/recursive_group_window.rs)).
The member's `name` and `kind` join the digested content and nothing outside it
distinguishes members, so two structurally different nominals carry different digests
(the per-declaration-distinctness dispatch keys on) while the same declaration
elaborated twice unifies. Because the digest unit is the component and not the
declaration group, a co-declared member that never references a sibling digests
independently, and a non-recursive member unifies with its standalone twin.

## Lift is a handle copy

A member handle is a `Copy` sixteen-byte digest that records nothing about the
registry that minted it, so lifting a nominal value out of a dying frame copies its
`ktype()` handle ([lift.rs](../../src/machine/execute/lift.rs)) — no set clone, no
visited-map traversal, no `Rc<CallFrame>` anchor. The registry owns the nodes and
outlives the run, so a handle stays dereferenceable; the cyclic composition edges
between members live in the registry, not on the value.

## Specificity stratification

Predicate arms
([`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs))
place a concrete member handle strictly below `OfKind(K)` of its own family, `OfKind(K)`
strictly below `OfKind(Proper)`, and that below `Any` in `is_more_specific_than`. Because
`OfKind` is type-channel-only, an `OfKind(K)` slot ranks against *type values* by
`kind_of` subsumption (`KKind::admits` / `KKind::strictly_below`), reading the member's
kind via `set.member(index).kind` — so each family ranks alongside the others with no
per-kind branching at the dispatcher. The signature
variant follows the parallel stratification: `KType::Signature { .. }` ≺
`KType::OfKind(KKind::Signature)` ≺ `Any`. This is the identity-and-wildcard slice of Layer 3 of the
[lookup → admit protocol](lookup-protocol.md); the predicate is the same one
every dispatch admit pass runs.

## Value carriers and the type / value partition

The [`KObject::Tagged`](../../src/machine/model/values/kobject.rs) carrier holds
`{ tag, value, identity }`, where `identity` is the value's own type handle — the
member's `SetMember` handle (or a `ConstructorApply` over it when an ascription
stamped a parameterized union's arguments in). It backs **both** every user-`UNION`
variant and the `TypeConstructor` family (`Result`, and the `CATCH` / `TRY` error
machinery); `ktype()` copies `identity`, so dispatch identity is one `u128`. Newtype
instances — scalar and record-repr — ride
[`KObject::Wrapped`](../../src/machine/model/values/kobject.rs), which carries a
`type_id: KType` (the member handle). A **signature** value rides the value
channel's `Type` arm as a `Signature` handle
([`Carried::Type`](../../src/machine/model/values/carried.rs)); the
identity is the carried handle itself rather than a synthesized shadow. A
**module** value rides the Object arm as `KObject::Module` — it is a value, not a
type identity.

`bindings.data` holds runtime instances, including module values. A value-position
reference to a nominal type token (passing `Outcome` to a constructor or ATTR call)
surfaces the
[`bindings.types` identity in the `Type` arm](../../src/machine/execute/dispatch/resolve_type_identifier.rs)
on demand via `Scope::resolve_type_identifier` — no
value-side schema carrier exists for newtype / union / Result.

## Unions dissolve into per-variant newtypes

A user `UNION` has no nominal family of its own: it is the anonymous-union join of
one `NewType` per variant — the sum-side counterpart of the struct → record-repr
`NEWTYPE` collapse.
`UNION Maybe = (Some :Number None :Null)` seals two
`KKind::NewType` members (`Some` over `Number`, `None` over `Null`) and binds `Maybe`
to the [union](../../src/machine/model/types/registry.rs) of those two member handles
([union.rs](../../src/builtins/union.rs)). Variant tags are capitalized
[`Type` tokens](tokens.md): the `UNION` schema field-list runs under
[`FieldNameKind::Type`](../../src/parse/triple_list.rs), so a lowercase tag is a
parse error — a variant name is a nominal type, not a field.

**A variant value is a `KObject::Tagged`** carrying its variant `tag`, the wrapped
`value`, and its member `SetMember` handle as `identity`, so its `ktype()` reports that
member handle. A slot typed `:(Maybe Some)` is that member handle; a `:Maybe` slot
is the anonymous union, which admits any member via
[`KType::matches_value`](../../src/machine/model/types/ktype_predicates.rs)'s
per-member delegation. Each member handle is strictly more specific than the union
(each member is a subtype of the union), so a variant-typed overload wins over a
union-typed sibling that admits the same value.

**The variant-reference surface is the union-qualified sigil `:(Maybe Some)`** —
a variant reached through its union, with no global `:Some` name and no `.`
path operator. The dispatcher's union constructor arm
([apply_callable.rs](../../src/machine/execute/dispatch/apply_callable.rs))
disambiguates by body shape: a bare `Type`-token body (`Maybe Some`) yields the
member handle type value, while a payload body (`Maybe (Some 42)`) constructs the
tagged variant value. An unknown variant name in either form is a schema error listing
the union's members. The member handle renders as its member name (`Some`), so
`:(Maybe Some)` round-trips.

**A schema field can name a sibling variant of the union still under seal** through
the same qualified sigil (`Node :(Tree Leaf)`): while `Tree` is the binder being
threaded, the elaborator
([typed_field_list.rs](../../src/machine/model/types/typed_field_list.rs)) recognizes a
sigil head naming that binder and folds `(Tree Leaf)` straight to a relative
`Sibling` reference rather than sub-dispatching — parking would deadlock on the very
seal awaiting this field — which the window seal
([recursive_group_window.rs](../../src/machine/model/types/recursive_group_window.rs))
rewrites to the sibling's absolute member handle like any intra-window reference. A bare
sibling tag (`Node :Leaf`) stays an unknown-type error: tags are never bare names, even
in the declaring schema.

**Nesting survives** because the tagged value holds its payload verbatim, so a
recursive union variant nesting another variant (`Succ (Zero null)`) keeps every layer.

**`MATCH` selects by type** ([match_case.rs](../../src/builtins/match_case.rs) via
[`find_branch_body_by_type`](../../src/builtins/branch_walk.rs)). A user-union variant
value and a `Result` are both `Tagged`, so a variant head admits by **tag-name
equality** against the value's own tag and binds the wrapped payload to `it` — a union's
sibling variants need no resolution, since the value carries its own tag. A general type
head resolves through the scope and binds the scrutinee
unchanged. Boolean-literal heads (`true ->` / `false ->`) and tag heads
settle first through an exact pre-pass that ranks strictly above every typed arm; the
remaining type heads admit by
[`matches_value`](../../src/machine/model/types/ktype_predicates.rs) and compete in the
same [`ExpressionSignature::most_specific`](../../src/machine/model/types/signature.rs)
tournament that resolves ordinary overload buckets, so the strictly most-specific
admitting arm wins and two arms with no strict winner are an ambiguity error (ruling
F1/F3). A head naming no type over a variant scrutinee errors listing the scrutinee's
variants. The winner's `it` reaches the arm frame through the same single-copy carrier
door `TRY`'s success arm uses — the scrutinee (or, for a variant/tag arm, its wrapped
payload) copied once at bind time, with no MATCH-specific bind site. See
[unions and match-by-type](type-language-via-dispatch.md#anonymous-union-sigil).

A `Result` value (`KKind::TypeConstructor`) carries the bare/applied ctor identity
(member handle / `ConstructorApply`) as its `Tagged` `identity`, so `TRY` selects arms
by error-tag string
([try_with.rs](../../src/builtins/try_with.rs) via `find_branch_body_by_tag`) — the same
tag-name dispatch a user union uses. See [error-handling.md](../error-handling.md).

## Type-only nominal install

NEWTYPE / UNION-named / Result finalize write **only** `bindings.types`: each builds
its identity (a member handle into its sealed component) and installs it through
[`Scope::register_type_upsert`](../../src/machine/core/scope.rs), which inserts if
absent and overwrites a `PartialEq`-equal entry, surfacing `Rebind` on a genuine
non-equal collision. The schema rides inside the member node, so construction reads
fields / variant types straight off the member's schema; there is no
second-namespace write to keep in sync.

**A declaration is identified by its stored `BindingIndex`.** Before installing,
[`finalize_nominal_member`](../../src/machine/model/types/resolver.rs) (and
`recover_union` for the union path) reads the committed `types[name]` entry
*with* the [`BindingIndex`](../../src/machine/core/bindings.rs) its installing
statement wrote, and decides three ways: an entry whose member is still unfilled
is this declaration's own seal pre-install, so the schema fills that set; an
entry installed at *this* statement's index is a parallel finalize of this same
declaration, short-circuited idempotently; anything else is a genuine prior
binding of the name, so the seal mints a fresh singleton and the install raises
`Rebind`. The index is the whole identity signal — a statement's lexical
position is unique within its scope, and the pre-install sits at index 0 below
every statement's own index, so the unfilled-member arm is what lets a
`RECURSIVE TYPES` block member (minted in the enclosing scope, sealed in the
child) reach its shared set. The single-home invariant —
Type-classed name lookups go through `Scope::resolve_type` only — holds because
the identity *is* the only entry.

`MODULE` is the exception that proves the rule: a module is a *value*, so it binds
into `bindings.data` through
[`Scope::bind_module`](../../src/machine/core/scope.rs) and nothing lands in
`types`. Its Type-classed name is resolved through the value channel by a bridge arm
in the resolver ladder (see
[modules.md § First-class modules](modules.md#first-class-modules)).

SIG installs the same way, through
[`Scope::register_type_upsert`](../../src/machine/core/scope.rs): a single
`KType::Signature { sig, pinned_slots }` identity in `bindings.types` serves
*both* roles. As a slot annotation (`er :Ordered`) it is the constraint form —
"any module satisfying Ordered"; as a value
(`KType::Signature { .. }` in the `Type` arm) it is the identity-bearing signature
carrier, carrying the live `decl_scope` via `sig`. The roles are disambiguated by
position, not by separate variants, so no value-side carrier is written;
`bindings.data` holds zero type carriers. Every nominal binder is a single
type-namespace write.

`LET <Type-class> = <module/sig/struct-value>` (e.g.
`LET Pt2 = Point`) installs the *original* type's identity
under the alias name rather than minting a fresh set — aliasing is
type-equivalent, so a slot typed by the alias dispatches to the same overload as
a slot typed by the original. Struct / union / module / Result / signature aliases
all route through `register_type` (type-only). Anonymous `UNION (...)` is not a
valid surface — every tagged value carries a real per-declaration identity.

## Schemas: members fill their slot at seal

Construction is two-phase. A scope-carried
[`RecursiveGroupWindow`](../../src/machine/model/types/recursive_group_window.rs) fixes
the group's membership and each member's `kind` up front and accumulates each member's
schema as it finalizes. Inside the window a member's schema names its siblings as
relative [`Sibling`](../../src/machine/model/types/node.rs) references — ordinary
interned content, resolved only against the ambient window.

At **seal** the window turns the relative schemas into interned member nodes
([recursive_group_window.rs](../../src/machine/model/types/recursive_group_window.rs)):

- It extracts each member's sibling references, partitions the members into
  strongly-connected components, and digests each component's condensation bottom-up —
  members in name order, intra-component references as relative indices, references
  outside the component folding the referent's finished digest as external content.
- It then interns each member as a
  [`TypeNode::SetMember`](../../src/machine/model/types/node.rs) with a
  [`NodeSchema`](../../src/machine/model/types/node.rs) (`NewType` repr — one per variant
  for a `UNION` — or a `TypeConstructor` schema + param names) whose every relative
  `Sibling` is rebuilt to the sibling's **absolute** member handle. Those handles form
  the cyclic composition edges; the registry holds the cycle without refcounting, so no
  sibling encoding is needed to break it. A sealed schema is read directly for
  construction, navigation, and matching — there is no projection step, because the
  absolute handles are already in place.

## `RECURSIVE TYPES` — the mutual-recursion construct

A self-recursive type needs no special construct: the binder threads its own name,
so a back-edge (a field naming the declaring type) lowers to a relative
`Sibling` reference and seals to the declaring member's own absolute handle in its
singleton component. Mutual recursion of two or more types *does* need a construct,
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

- discovers each member's `(name, kind)` from the body declarations, opens one
  shared `RecursiveGroupWindow` announcing that membership, and dispatches the
  declarations against a child scope carrying the window
  ([`Scope::child_recursive_group`](../../src/machine/core/scope.rs) /
  [`nearest_recursive_window`](../../src/machine/core/scope.rs));
- inside the block every member name is threaded, so a cross-reference lowers to a
  relative `Sibling` reference against the window rather than minting a standalone
  type;
- on the block's dep-finish, guarantees resolution — every member must have filled
  its window slot, or a forward reference named a name outside the group, raised as a
  localized shape error at the block boundary. The window then seals: it partitions the
  members into SCCs and interns each as a `SetMember`. The sealed members mirror into
  the enclosing scope as member handles (`A`, `B` bind as ordinary type
  names), and the group name (`Pair`) binds a `TypeNode::Group` handle.

`RECURSIVE TYPES` is the only way to express mutual recursion of two or more
types; the block scopes its threaded group within strict lexical order, so a
forward reference is either discharged into the set or a localized error, never a
placeholder that survives into a sealed type.

## `NEWTYPE` and the `Wrapped` carrier

`NEWTYPE Distance = Number` declares a fresh nominal identity over a transparent
representation. Declaration seals a singleton component whose one member is a
[`NodeSchema::NewType`](../../src/machine/model/types/node.rs) over the repr handle
and writes only `bindings.types` — the same type-only shape NEWTYPE / UNION
/ Result use. The repr is not part of identity. A record repr
(`NEWTYPE Point = :{x :Number, y :Number}`) is a `NodeSchema::NewType` over a
`Record` node — the product-side nominal form; `.x` reads the field through ATTR's
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
  relative `Sibling` reference and seals — through the group window
  ([recursive_group_window.rs](../../src/machine/model/types/recursive_group_window.rs);
  a `UNION` additionally maps the union's own name to the join of its variant members,
  ruling F2) — to the declaring member's own absolute back-edge in its singleton
  component. A `:(LIST OF Self)` field threads the same way, sealing `List` over that
  back-edge, and a nested record field type
  (`:{inner :{owner :Node}}`) elaborates inline through the same walker so it threads
  too. A `NEWTYPE` member of a `RECURSIVE TYPES` block routes through this path,
  filling the block's shared window rather than opening its own.

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
the `type_id` handle (a `SetMember` of a `Newtype`-kind member for construction, an
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

## Constructor families: `NEWTYPE (Type AS Wrapper)`

`NEWTYPE (Type AS Wrapper)` declares a **type-constructor family** — the koan-source
counterpart of the higher-kinded slot form `TYPE (Type AS Wrap)`
([functors.md § Higher-kinded type slots](functors.md#higher-kinded-type-slots)). It is
declaration-by-example: the head mirrors the application surface with the concrete
arguments replaced by the parameter names. The declarator
([`body_constructor_family`](../../src/builtins/newtype_def.rs)) reuses the shared `TYPE`
declaration parser, so one or more parameters may be declared and a repeated name is a
shape error. It is
valid in any scope — top level or a `MODULE` body — with no SIG-body gate, so a module can
declare the constructor member a higher-kinded signature slot demands.

**Identity is a singleton `TypeConstructor` set at the declaring scope.** The
declaration mints one `KKind::TypeConstructor` member —
[`mint_type_constructor`](../../src/builtins/newtype_def.rs), an empty variant schema plus
the declared `param_names` — and writes it to `bindings.types` only, no value-side carrier.
What separates a NEWTYPE-declared family from the `TYPE` declarator's abstract constructor
slot is the *node*: the slot is an [`AbstractType`](../../src/machine/model/types/node.rs)
with non-empty `param_names`, which names a kind and constructs nothing, while a family is
a `SetMember` and constructs values.
The empty schema is the second discriminant, separating a constructor family from the
builtin `Result`, whose non-empty variant schema routes construction down the sealed
tagged-union path instead.

**The family is the identity-wrapper over its argument** — `(Elem AS Wrapper)` is a newtype
over `Elem` itself, so the applied argument *is* the representation; there is no
type-variable substrate. Application binds the parameter by name —
`:(Wrapper {Type = Number})`, or the arity-1 sugar `:(Number AS Wrapper)` — and lowers to
`ConstructorApply { constructor: <the Wrapper member handle>, arguments: {Type = Number} }`, the same
lowering an abstract constructor slot's application uses.

**Construction stamps then collapses.** `Wrapper (v)` routes through
[`dispatch_construct_apply`](../../src/machine/execute/dispatch/constructors.rs) (an
[`ApplyConstructor`](../../src/machine/execute/dispatch/single_poll.rs) `CtorKind`), which
mirrors `dispatch_construct_newtype`'s arity handling: a single redundant paren group
unwraps, an empty body is `ArityMismatch { expected: 1, got: 0 }`. Its `finish_witnessed`
arm reads the resolved value `v`, **stamps** `v`'s full `ktype()` — including a `Wrapped`
payload's own nominal identity — as the sole applied arg, then **collapses** by peeling one
`Wrapped` layer off `v` so the stored `inner` is never itself `Wrapped` (the single-layer
invariant the constructor path holds; the peeled identity is preserved *in the stamped
arg*). The result is `KObject::Wrapped { inner, type_id: ConstructorApply(<ctor member handle>,
{<param> = arg}) }` — the family's sole parameter names the stamped arg — so the value's
`ktype()` reports the applied type for free and inhabits
`:(<v's type> AS Wrapper)`. A record-literal payload (`Wrapper ({x = 1.0})`) rides through
as a single positional value; ATTR then projects a field through the `Wrapped` layer.
Value construction is arity-1 by nature — one wrapped value infers one argument — so
constructing over a family declaring two or more parameters is a shape error naming the
arity; such a family is applied in type position only.

**Matching keys on the ctor nominal plus per-name agreement.** A slot typed
`:(Number AS Wrapper)` is a `ConstructorApply` slot; a value satisfies it when the two
ctors' member handles are equal (a `u128` compare) and the two argument
records name the same parameters, each stamped arg agreeing with its same-named slot arg —
an `Any` slot arg admits anything, otherwise the args must be structurally
equal. The rule lives in one helper
([`constructor_apply_admits`](../../src/machine/model/types/ktype_predicates.rs)) shared by
both the `KType::matches_value` `Wrapped` arm and the
`KType::accepts_carried` dispatch arm — types are owned, so neither arm constrains the
value's lifetime — so a FN parameter typed `:(Number AS Wrapper)` and a
value-position match apply the identical admission. Two `Wrapper (v)` values compare `==`
through the ordinary `Wrapped` structural-equality path.
