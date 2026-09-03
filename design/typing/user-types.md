# User-declared nominal types

A `NEWTYPE` or named `UNION` declaration is a *nominal* type: its
identity is its declaration, not its shape. Nominal identity is content-addressed
in the run-frame registry ([type-registry.md](type-registry.md)): each member is an
interned [`TypeNode::SetMember`](../../src/machine/model/types/node.rs) whose
identity unit is its own *strongly-connected component* under the sibling-reference
relation — not its declaration group. A non-recursive type is a singleton
component; a self-recursive type, an `A ↔ B` pair, or a longer cycle is one
component of several members.

Two node kinds carry the model:

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
in [the declared builtin names](../../src/machine/model/types/builtin_names.rs)
`KType::from_symbol` reads).

Signatures live in their own KType variant —
[`KType::Signature { schema, .. }`](../../src/machine/model/types/ktype.rs)
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
presentation — the numeric order of the members' name symbols
([label-interning.md](../label-interning.md)), with the owning binder (a `UNION`'s name,
for a variant) as a tiebreak the digest never sees, so a module-hosted variant digests
identically to its standalone twin and two same-tag variants of different binders
take stable distinct positions. The digest is minted at seal from finished content, never
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

Every nominal wrap rides one carrier:
[`KObject::Wrapped`](../../src/machine/model/values/kobject.rs), which carries the
payload and a `type_id: KType` — the member's `SetMember` handle, or a
`ConstructorApply` over it when an ascription stamped a parameterized type's
arguments in. That is newtype instances — scalar and record-repr — every
user-`UNION` variant value, and the `TypeConstructor` family (`Result`, and the
`CATCH` / `TRY` error machinery); `ktype()` copies `type_id`, so dispatch identity
is one `u128`. A **signature** value rides the value
channel's `Type` arm as a `Signature` handle
([`Carried::Type`](../../src/machine/model/values/carried.rs)); the
identity is the carried handle itself rather than a synthesized shadow. A
**module** value rides the Object arm as `KObject::Module` — it is a value, not a
type identity.

`bindings.data` holds runtime instances, including module values. A value-position
reference to a nominal type token (passing `Outcome` to a constructor or ATTR call)
surfaces the
[`bindings.types` identity in the `Type` arm](../../src/machine/execute/decide/resolve_type_identifier.rs)
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

**A variant value is a `KObject::Wrapped`** like any newtype instance, carrying its
payload and its member `SetMember` handle as `type_id`, so its `ktype()` reports that
member handle. A slot typed `:(Maybe.Some)` is that member handle; a `:Maybe` slot
is the anonymous union, which admits any member via
[`KType::matches_value`](../../src/machine/model/types/ktype_predicates.rs)'s
per-member delegation. Each member handle is strictly more specific than the union
(each member is a subtype of the union), so a variant-typed overload wins over a
union-typed sibling that admits the same value.

**The variant-reference surface is member projection through the owning union** —
`Maybe.Some` in expression position, `:(Maybe.Some)` under the type sigil — the same
ATTR type-member projection a signature member uses
([attr.rs](../../src/builtins/attr.rs)). The two spellings are the same *shape*, not just the
same meaning: a tag is Type-class, so the parser already wraps the projection in a
`SigiledTypeExpr`, and an explicit sigil collapses onto that node instead of layering a second
one. A tag is a schema member of its union the
way a field is a schema member of its record, never a bare scope name: `:Some`
resolves nowhere, so two binders may own the same tag. The projection yields the
member handle as a type value, and ordinary application of it constructs —
`Maybe.Some 42` wraps the payload under the member's identity. An unknown member
name is a schema error listing the union's members. The member handle renders as its
member name (`Some`).

**The member-lookup rule reads the member list, never the chain**
([union.rs](../../src/builtins/union.rs)`::union_member`) — the same probe a
`MATCH … OVER` arm head takes, so the two surfaces can never disagree about what
names a variant. A `SetMember` — what a `UNION` mints per tag — answers the field
whose bare symbol bits match its declared name. A **structural** member, what an
inline `:(Number | Str)` holds, declares no name at all: it answers only a field
that resolves in the *reading scope* to a type whose handle is that member
(`NumStr.Number`). A union mixing the two answers a name from whichever shape
holds it.

**A schema field can name a sibling variant of a union still under seal** through
the same member projection (`Node :(Tree.Leaf)`): when `Tree` is a binder of the active
declaration window, the elaborator
([typed_field_list.rs](../../src/machine/model/types/typed_field_list.rs)) recognizes
the projection head and folds `Tree.Leaf` straight to that variant's handle rather than
sub-dispatching — parking would deadlock on the very seal awaiting this field. The
handle is relative while the window is open, and the seal rewrites it to the sibling's
absolute member handle like any intra-window reference. Because the window is the
module body's when the body announced the binder, this reaches across statements: a
co-declared `NEWTYPE` may type a field by a sibling union's variant pre-seal. A bare
sibling tag (`Node :Leaf`) stays an unknown-type error: tags are never bare names, even
in the declaring schema.

**Nesting survives** wherever the payload's identity differs from the variant's declared
repr: a recursive union variant wrapping another variant (`Nat.Succ (Nat.Zero null)`)
holds its payload verbatim and keeps every layer. A variant whose repr *is* a sibling
member (`Node :(Tree.Leaf)`) takes the ordinary re-tag instead, folding the redundant
layer — the same peel-or-hold rule every newtype construction runs (see
[`NEWTYPE` and the `Wrapped` carrier](#newtype-and-the-wrapped-carrier) below).

**`MATCH` has two regimes, split by the form's syntax**
([match_case.rs](../../src/builtins/match_case.rs) via
[branch_walk.rs](../../src/builtins/branch_walk.rs)). `MATCH <expr> OVER <U> WITH
(…)` is *member elimination*: `U` resolves to a union type, every arm head names a
member of `U` — or is `_`, the **default arm** — and no member and no `_` is named
twice. An unknown head errors listing the members. Coverage is required *unless* a `_`
arm stands in: without one, a missing member is an error at the form; with one, the
arm set may leave members uncovered and `_` runs for a value of any member no named
arm claims. A named arm always wins over `_`, whatever the source order. All of this
is checked before any value is read, so no arm order and no runtime value can leave
the match without a body. `_` defaults the union's uncovered *members* only — a
scrutinee inhabiting no member of `U` is still its own error naming both, `_` present
or not. The winning arm — named or `_` — binds
`it` to the matched member's payload — the value under the member's wrap — with a
non-wrapping member binding the value itself. `OVER` takes any union-noded operand:
a `UNION` binder, a `LET`-bound alias of an anonymous union, or an inline
`:(A | B)`. An `OVER`-less `MATCH` is a *type test*: every head resolves through
the scope, boolean-literal heads (`true ->` / `false ->`) settle first through an
exact pre-pass that ranks strictly above every typed arm, the remaining heads admit
by [`matches_value`](../../src/machine/model/types/ktype_predicates.rs) and compete
in the same
[`ExpressionSignature::most_specific`](../../src/machine/model/types/signature.rs)
tournament that resolves ordinary overload buckets — the strictly most-specific
admitting arm wins, and two arms with no strict winner are an ambiguity error
(ruling F1/F3) — and the winning arm binds `it` to the scrutinee unchanged: a test,
not an elimination. Which regime reads a head is never a property of the runtime
scrutinee. `it` reaches the arm's overlay scope through the same single-copy carrier
door `TRY`'s success arm uses — copied once at bind time, with no MATCH-specific
bind site. See
[unions and match-by-type](type-language-via-dispatch.md#anonymous-union-sigil).

`TRY` selects arms through the same member walk with a fixed member set — `Ok` plus
the members of the prelude `KError` union, an implicit `OVER`
([try_with.rs](../../src/builtins/try_with.rs)). The two forms share one arm parser
and the `_` default-arm rule; `TRY` drops the coverage requirement, because an
unhandled kind re-raises rather than failing the form — see
[error-handling.md](../error-handling.md).

**A union head takes named type arguments** — `:(Maybe {Some = Number})`,
`:(Result {Ok = Number, Error = Str})`. Each argument name must name a member, and
the application lands per member ([union.rs](../../src/builtins/union.rs)): the
result is the union of the members with every named one replaced by the
`ConstructorApply` over it, so a member no argument names simply rides bare and
partial application is legal. A record body on a union head is therefore always
*type application*, never construction — a variant is reached by projection alone.
The admission and stamping rules for the applied form are in
[ktype/parameterization-and-variance.md § Applying a union head](ktype/parameterization-and-variance.md#applying-a-union-head).

## Type-only nominal install

NEWTYPE / UNION-named / Result finalize write **only** `bindings.types`: each builds
its identity (a member handle into its sealed component) and installs it through
[`Scope::register_type_upsert`](../../src/machine/core/scope.rs), which inserts if
absent, overwrites idempotently when the same declaration re-enters, and surfaces
`Rebind` on a collision with a different declaration. The schema rides inside the
member node, so construction reads fields / variant types straight off the member's
schema; there is no second-namespace write to keep in sync.

**A declaration is identified by the statement that installed it — an identity
Koan mints for itself.** Each `types` entry
stores, beside its type, a [`DeclarationSite`](../../src/machine/core/bindings.rs): the
[`Installer`](../../src/machine/core/bindings.rs) naming the statement that installed it,
paired with its lexical [`BindingIndex`](../../src/machine/core/bindings.rs).
The installer alone answers the same-declaration question:
[`finalize_nominal_member`](../../src/machine/model/types/resolver.rs) installs through
[`register_type_upsert`](../../src/machine/core/scope.rs), which overwrites when the
installing statement matches the stored entry's — one declaration re-entering, i.e. a
parallel finalize whose re-elaboration cannot differ — and raises
`Rebind` on any other. Content plays no part: a byte-identical redeclaration in
one scope is submitted as a distinct statement and is a `Rebind`, and so is a re-run of the
same declaration text over a persistent scope, whose
[`StatementId`](../../src/machine/core/statement_id.rs) is minted from a never-recycled
process-global counter and so can never collide with an earlier run's. Nothing here
names a scheduler slot or edge: the counter is Koan's own, so the scheduler's
index-recycling policy is not load-bearing for redeclaration semantics
([scheduler-library.md](../scheduler-library.md)). The `BindingIndex` in the entry has one job
left — the visibility gate `idx < cutoff` reads it, against the statement
position the submission's driver declared. The
single-home invariant — Type-classed name lookups go through `Scope::resolve_type` only
— holds because the identity *is* the only entry.

`MODULE` is the exception that proves the rule: a module is a *value*, so it binds
into `bindings.data` through
[`Scope::bind_module`](../../src/machine/core/scope.rs) and nothing lands in
`types`. Its Type-classed name is resolved through the value channel by a bridge arm
in the resolver ladder (see
[modules.md § First-class modules](modules.md#first-class-modules)).

SIG installs the same way, through
[`Scope::register_type_upsert`](../../src/machine/core/scope.rs): a single
`KType::Signature { schema, .. }` identity in `bindings.types` serves
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
valid surface — every variant value carries a real per-declaration identity.

## Schemas: members fill their slot at seal

Construction is two-phase. A **declaration window** fixes the group's membership and
each member's `kind` up front and accumulates each member's schema as it finalizes.
Inside the window a member's schema names its siblings as
relative [`Sibling`](../../src/machine/model/types/node.rs) references — ordinary
interned content, resolved only against the ambient window.

A window comes in two representations, differing only in who owns it:

- The **ambient** [`AnnouncedWindow`](../../src/machine/model/types/declaration_window.rs)
  rides a module body's child scope, holding the names that body announced (below).
  It is `Drop`-free and region-bumped — every field a `Copy` run or a `Cell` of one —
  so a scope can carry it for free; its members are always `NewType`-schema'd and its
  member set is fixed at the scan, which is what makes that representation possible.
- The **declarator-local**
  [`RecursiveGroupWindow`](../../src/machine/model/types/recursive_group_window.rs) is
  opened and sealed inside one declaration — a standalone `NEWTYPE` or `UNION`, a
  generative `:|` mint. It carries `TypeConstructor` schemas and grows by threaded
  discovery, neither of which the ambient form needs, and it never rides a scope.

Consult paths read either through one borrowed
[`WindowView`](../../src/machine/model/types/declaration_window.rs); a declarator holds
its window for the whole declaration as a `DeclWindow`. Both seal through the one pure
core, [`seal_group`](../../src/machine/model/types/recursive_group_window.rs).

At **seal** the window turns the relative schemas into interned member nodes:

- It extracts each member's sibling references, partitions the members into
  strongly-connected components, and digests each component's condensation bottom-up —
  members in name-symbol order (owner as a non-digested tiebreak), intra-component
  references as relative indices, references
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

## Mutual recursion — the module-body announcement

A self-recursive type needs no construct: the binder threads its own name, so a
back-edge (a field naming the declaring type) lowers to a relative `Sibling`
reference and seals to the declaring member's own absolute handle in its singleton
component. Mutual recursion of two or more types *does* need one, because type names
obey strict source order (see [elaboration.md](elaboration.md)): in a bare
`NEWTYPE A = :{b :B}` / `NEWTYPE B = :{a :A}` pair, whichever is written first
forward-references the other, a position error.

The construct is a **module body**. Before any body statement runs, `MODULE`
pre-announces the body's top-level type declarations
([`announce_type_members`](../../src/machine/model/binder.rs)), and that announcement is
the ambient `AnnouncedWindow` the whole body elaborates against — so the two
declarations are co-declared members of one window and each cross-reference is a
`Sibling` back-edge:

```
MODULE pair = (
  NEWTYPE Aa = :{b :Bb}
  NEWTYPE Bb = :{a :Aa}
)
```

`GROUP` inherits it unchanged — a group *is* a module
([`group_def.rs`](../../src/builtins/group_def.rs)). Announcement stays a module
property: the program's own top level announces nothing, so a top-level cycle takes
the module wrapper and is otherwise an ordinary forward-reference miss.

**What announces.** A statement announces iff its own parse-time binder key matches
the `NEWTYPE <Name> = _` or `UNION <Name> = _` spec in
[`BINDER_SPECS`](../../src/machine/model/binder.rs) — the *full* bucket key, every
keyword pinned in position, so a user overload that merely shares a head keyword
announces nothing and the constructor-family key `NEWTYPE (<Type> AS <Name>)` is
excluded structurally. The boundary is the body's top-level statement split, the same
one `GROUP` reads its operator members off; a declaration nested inside another
statement's slot, or computed at run time, is not announced and keeps ordinary
dataflow order. Declaring one name twice is a shape error raised by the scan, before
any statement runs.

**Union variants are owned members.** A `UNION` announces one member per statically
scannable variant tag, each tagged with its owning binder. An owned member is never
bare-name-resolvable and never lands in `bindings.types`: it is reached only through
its binder (`:Tree`) or through member projection (`:(Tree.Node)`), whose lookup is
scoped by that binder's own member list — so two binders in one body may own the same
bare tag. The binder itself is not a member; it denotes the union of the members it
owns. A standalone `UNION` is the one-binder special case of the same machinery.
Because variants join the module window's SCC computation, a `UNION` ↔ `NEWTYPE`
cycle co-seals.

**Announced names are visible body-wide, and what they resolve to depends on who
asks** ([`resolver.rs`](../../src/machine/model/types/resolver.rs)):

- A **declarator** — the statement building the member's own schema — takes the
  relative `Sibling` handle (or, for a binder name, the union of its members'
  siblings). Parking it until the window sealed would deadlock the group on its own
  producer. A declarator that sub-dispatches part of its schema (a `:(LIST OF Rest)`
  sigil repr) threads the announced names in as pre-resolved cells first, so the
  window-less standalone dispatcher never has to answer for them.
- A **consumer** — an FN signature, a `LET` ascription, any ordinary type position —
  must never observe a relative handle, which is meaningless outside the window that
  minted it. It parks on the producers of *every* still-unfilled member (a variant's
  producer is its owning binder's, since a variant stamps none of its own), so one
  wake lands after the seal, and then reads the member's **absolute** handle straight
  off the sealed window. An unfilled member whose producer is gone is a declaration
  that died: a typed miss, never a park that would never wake.

**The seal fires at the last member's fill**, not at module close, so a parked
consumer wakes as early as the identities exist. The statement whose fill closes the
group carries the `types` writes for the whole group — one per standalone member and
one per binder, variants excluded — at that statement's declaration site. Module close
then only checks the belt: a window still open there is a scan/dispatch disagreement,
surfaced as a typed `ShapeError` naming the members that never sealed, never a hang.

Because identity is the SCC and not the declaration group, announcing a whole module
body is identity-neutral: a co-declared member that references no sibling digests
independently and unifies with its standalone twin.

The module surface — how the announced members reach a use site — is described in
[modules.md § Module bodies announce their type declarations](modules.md#module-bodies-announce-their-type-declarations).

## `NEWTYPE` and the `Wrapped` carrier

`NEWTYPE Distance = Number` declares a fresh nominal identity over a transparent
representation. Declaration seals a singleton component whose one member is a
[`NodeSchema::NewType`](../../src/machine/model/types/node.rs) over the repr handle
and writes only `bindings.types` — the same type-only shape NEWTYPE / UNION
/ Result use. The repr is not part of identity. A record repr
(`NEWTYPE Point = :{x :Number, y :Number}`) is a `NodeSchema::NewType` over a
`Record` node — the product-side nominal form; `.x` reads the field through ATTR's
`Wrapped` fall-through over the record repr.

**A record repr also answers a field's *type*, under the type sigil.** `:(Point.x)` off the
`Point` type — as opposed to off a `Point` value — projects the field's declared type as a type
value, the product-side peer of a signature's `VAL`-slot read
([attr.rs](../../src/builtins/attr.rs)`::access_type_member`, which carries the repr handle out
of the lhs node's registry borrow and reads the `Record` node in a second one). The projected
handle *is* the declared type, so a slot spelled `:(Point.x)` admits exactly what a slot spelled
with that type admits, and the read chains where the field is itself record-shaped
(`:(Point.inner.x)`). A `LET`-bound alias of the nominal resolves to the same handle and projects
identically.

The sigil is required because `x` is a value token
([tokens.md § A value token names a type only under the sigil](tokens.md#a-value-token-names-a-type-only-under-the-sigil)):
bare `Point.x` off the type names no *value*. The miss splits on whether the schema carries the
field: one the schema does declare is not "no member" — that would contradict the hint that
follows — so it reports the field as a declaration and points at the sigiled spelling that names
its type, while a field the schema does not carry is the plain memberless miss, which no spelling
would answer. Under the sigil an unknown field lists the record's fields
the way a union member miss lists its variants, and a scalar repr (`NEWTYPE Meters = Number`) has
no fields to name and takes the memberless-type error in either context.

The [`NEWTYPE`](../../src/builtins/newtype_def.rs) declarator carries three overloads
selected by the repr part-kind:

- A **scalar / bare-leaf** repr (`= Number`, `= Foo`) rides the `OfKind(ProperType)` slot,
  which is a plain kind expectation: the dispatch lane resolves the name and the body seals a
  plain singleton Newtype over the `KType` it receives. The slot registers the role `NEWTYPE
  repr`, so a name that binds to nothing still reports ``NEWTYPE repr `Nope` is not a known
  type`` from the lane's raise. The exception is a name still finalizing in `NEWTYPE`'s own
  declaration group: that operand is exempt from the dispatch-time park, which would deadlock the
  group, so it arrives raw and the body resolves-or-awaits it
  ([slots-and-signatures.md § Type-position slot kinds](ktype/slots-and-signatures.md#type-position-slot-kinds)).
- A **non-record sigil** repr (`= :(LIST OF Elem)`) rides a `:SigiledTypeExpr` slot that
  captures the sigil *raw* — more specific than `OfKind(ProperType)`, so it wins with no
  admission-rule change. The shared `body` threads the declarator's window names into
  the captured sigil as pre-resolved cells
  ([`rewrite_window_refs`](../../src/machine/model/types/typed_field_list.rs)), then
  sub-dispatches it to a resolved `KType` and seals a plain Newtype over it — the
  sub-dispatch runs on the window-less standalone dispatcher, so a co-declared
  reference has to be resolved before the node leaves.
- A **record** repr (`= :{…}`) rides a distinct `:RecordType` slot — the sibling of
  `:SigiledTypeExpr`, also more specific than `OfKind(ProperType)` — routed to its own
  `body_record_repr` overload. Capturing the field list raw lets the declarator own its
  elaboration: it threads the binder name
  ([`Elaborator::with_threaded`](../../src/machine/model/types/resolver.rs)) through
  [`parse_typed_field_list_via_elaborator`](../../src/machine/model/types/typed_field_list.rs),
  so a self-reference (`NEWTYPE Node = :{value :Number, next :Node}`) lowers to a
  relative `Sibling` reference and seals — through the declaration window
  ([declaration_window.rs](../../src/machine/model/types/declaration_window.rs); a
  binder name additionally maps to the join of the variant members it owns) — to the
  declaring member's own absolute back-edge in its singleton
  component. A `:(LIST OF Self)` field threads the same way, sealing `List` over that
  back-edge, and a nested record field type
  (`:{inner :{owner :Node}}`) elaborates inline through the same walker so it threads
  too. A `NEWTYPE` announced by its module body routes through this same path, filling
  the body's ambient window rather than opening its own.

Construction (`Distance(3.0)`, `Bar(Foo(3.0))`) flows through
[`type_call`](../../src/machine/execute/decide/single_poll.rs)'s `Newtype` arm —
which branches on the resolved member's `kind` — into
[`newtype_def::newtype_construct`](../../src/builtins/newtype_def.rs), which
schedules the value sub-expression via `dispatch_in_scope` and waits on it via a
dep-finish whose finish closure type-checks against `repr` and produces a
[`KObject::Wrapped { inner: &'a PayloadSubstrate<'a>, type_id: KType }`](../../src/machine/model/values/kobject.rs)
carrier — the `inner` payload is a region-resident single-cell substrate
(`PayloadSubstrate`) born through the fold door.

**The wrap chooses peel-or-hold by the payload's identity.** Two door verbs record
the wrapper's intent, each allocating the payload substrate through the enclosing
fold. A **re-tag** — the constructed value's identity is exactly this repr, e.g.
`Bar(some_foo)` where `some_foo` is already a `Foo` and `NEWTYPE Bar = Foo` — takes
[`KObject::wrapped_peel`](../../src/machine/model/values/kobject.rs), collapsing one
`Wrapped` layer so identities never stack (a `Wrapped` payload rides its inner
substrate borrow verbatim). A **genuine construction** — the payload is a *member* of
the type being built, whose identity differs from the repr, e.g. a `UNION` variant
`Succ :Nat` wrapping another `Nat` variant — takes
[`KObject::wrapped_hold`](../../src/machine/model/values/kobject.rs), preserving the
payload verbatim so the recursion the dissolved-union model needs survives
(`Succ (Zero null)` keeps both layers). `check_newtype_repr` decides which by comparing
the payload's `ktype()` to the projected `repr` before the witness build.

The construction path is driven from the `type_call` fast lane (which resolves the
verb through `scope.resolve_type_with_chain` first and branches on the resolved
member's `kind`) rather than a registered builtin sharing the `[OfKind(ProperType), …]`
signature bucket — a sibling primitive on that bucket would re-dispatch infinitely.

The `Wrapped` carrier also backs **opaque VAL-slot identities**: an opaque
ascription's coercion walk re-tags each member the SIG types by an abstract member
through this same `wrapped_peel` collapse, so the view's scope is born holding
values that report the abstract type rather than its representation and a slot read
patches nothing. The two uses share the variant — distinguished by the `type_id`
handle (a `SetMember` of a `Newtype`-kind member for construction, an
`AbstractType` or a `ConstructorApply` over the view's per-call mint for a coerced
slot) — and the same collapse and ATTR fall-through rules apply to both. See
[modules.md § VAL-slot reads carry the abstract member identity](modules.md#val-slot-reads-carry-the-abstract-member-identity).

ATTR over a `KObject::Wrapped` falls through to `inner` via
[`access_field`'s `Wrapped` arm](../../src/builtins/attr.rs). A runtime `Wrapped` lhs is
matched by a *type*, never by a kind: it lands in the least-specific `s: Any` ATTR
overload, and `access_field` validates the shape in the body, descending one level per
access.
Specificity (`Any` ≺ `OfKind` ≺ `Identifier`) keeps this unambiguous with the
sibling overloads: an `Identifier` lhs wins `body_identifier`, a module / type-token lhs
wins its `OfKind` overload, and only a bare runtime value falls through here. Missing-field
diagnostics name the inner record (`b: Boxed = Point; b.z` reports the field miss on
`Point`) — the fall-through is transparent at the diagnostic level too.

The nominal layer is what the fall-through *adds*, not what makes a record readable: a
bare `KObject::Record` — an anonymous record value with no `NEWTYPE` anywhere — projects
a field off the same `RecordSubstrate` one layer shallower, so `person.name`, `ATTR person
"name"` and `ATTR person (which)` all reach the same cell. Which fields that read admits is
decided by the value's **carried record type**, the same currency dispatch reads by, not by
the substrate's physical layout: a `FROM` projection shares the substrate whole and narrows
only the carried type, so a projected-away field is unreadable through the view
([type-language-via-dispatch.md § `FROM`](type-language-via-dispatch.md)). A miss on a bare
record renders the structural type it carries (`` `:{x :Number y :Number}` has no field `z` ``),
the same `ShapeError` shape a newtype's miss reports under its nominal name. A value with no
fields at all — a NEWTYPE over `Number`, a bare scalar — falls to the catch-all, which names
the field and the operand's rendered type rather than the builtin's `s` slot. The nominal-family
keyword `Newtype` is *not* registered in
[`KType::from_symbol`](../../src/machine/model/types/ktype_resolution.rs)'s table; the `OfKind(Newtype)`
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
builtin `Result`, whose members route construction down the sealed
union-member path instead.

**The family is the identity-wrapper over its argument** — `(Elem AS Wrapper)` is a newtype
over `Elem` itself, so the applied argument *is* the representation; there is no
type-variable substrate. Application binds the parameter by name —
`:(Wrapper {Type = Number})`, or the arity-1 sugar `:(Number AS Wrapper)` — and lowers to
`ConstructorApply { constructor: <the Wrapper member handle>, arguments: {Type = Number} }`, the same
lowering an abstract constructor slot's application uses.

**Construction stamps then collapses.** `Wrapper (v)` routes through
[`dispatch_construct_apply`](../../src/machine/execute/decide/constructors.rs) (an
[`ApplyConstructor`](../../src/machine/execute/decide/single_poll.rs) `CtorKind`), which
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

