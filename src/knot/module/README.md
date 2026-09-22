# Modules as values

The layer that reads a module by name: the views `:|` and `:!` build, the
coercion that births a view's members, and the binding a `USING … SCOPE` block
enters on.

A module is one kind of [knot](../README.md) node, holding its self-signature
and its members. This module owns that node — its payload, its doors and
everything that reads one by name — beside the [function](../function.rs) node
its sibling owns, which it never names. It reaches the shared node vocabulary
through [`knot`](../README.md#what-sits-where) and rests on
[`values`](../../values/README.md), [`scope`](../../scope/README.md),
[`elaborate`](../../elaborate/README.md) and the
[type lattice](../../type_lattice/README.md).

What it does *not* do is evaluate anything. `m :| Sig`, `m.f`, a `USING`
expression and a call through a barrier are
[module programs](../../../roadmap/rewrite/modules.md)'; the doors here are what
that item will drive, and they are exercised over activations bound by hand.

## Birth

A module binder comes into being body-first ([birth.rs](birth.rs)). The caller
builds the body's activation with `body_activation`, runs it until every slot is
bound, and only then ties the binder through `tie_member`: a module's type and
weight are facts about the members its body binds, and a knot node is written
once. A module is alone in its component — a mention reached from a module
binder's root is eager whatever body it sits in, so a module is never in a cycle
— so it shares nothing with the [tie](../README.md#the-tie)'s staging and ties
on its own path. `Module::tie` is the one private-field door a body-born module
and a view both go through, so the two have one representation and one copy.

## Layout order

Every door here rests on one rule, and [`layout.rs`](layout.rs) is the only
place it is spelled:

> **Value members, sorted by name; then type members, sorted by name**, abstract
> and manifest merged into one run.

Three readers agree on it, and none consults the others:

- a **body shape** lays its value slots out first and its type slots after, each
  channel symbol-sorted ([two channels](../../scope/README.md#two-channels)), so a
  body-born module's member run is its finished activation's slots read out in
  slot order, with no remap;
- a **signature**'s member tables are symbol-sorted by name, so a view's member
  run is built by walking them;
- a **`USING` block**'s parameters are the surfaced names, which the shape
  builder sorts the same way.

So member `k` of a channel is slot `k` of that channel everywhere, and `m.f` is
an index rather than a search. The sort is by interned symbol — a content digest
— not by the text of a name, so nothing may read the order as alphabetical.

`layout` exposes the rule as `value_count`, `member_count`, `type_members`,
`member_index`, `schema_of` and `member`, the last being what `m.f` reads: the
member's index in the schema, then that index into the node's run.

## The view door

[`ascribe`](view.rs) is what `m :! Sig` and `m :| Sig` build. A view is a module
of its own — the same node, through the same constructor a body-born one goes
through — holding only the members its signature names, at the types that
signature declares. So a view and a body-born module have one representation and
one copy, and nothing downstream asks which it holds.

The door reads the source's self-signature, checks it satisfies the ascribed one
(`sig_subtype`), and then fixes two substitutions over the signature's abstract
members:

- `from` — what the *source* binds each abstract member to, which is the
  substitution the member it holds was built under;
- `to` — under `:!`, `from` itself; under `:|`, a **fresh mint per abstract
  member**: a rigid variable carrying this application's nonce, over the
  declaration's own parameter names and bound.

Everything after that is one body, [`build`](view.rs), so the two operators are
not two paths that agree: `:!` is the case of `:|` where `to` is `from`, and
every coercion below stops at its first comparison. Two `:|` ascriptions of one
signature over one module therefore produce views whose carriers do not unify,
while `:!` is a relabelling.

The view carries a signature of its own — every type member fixed manifest at
what `to` gives it, every value slot at its declared type read under `to` — so a
module's own signature never has abstract members, and a view's is a module's.

An `Unascribable` names which door refused: the operand is no module, the
ascribed handle names no signature, the module does not satisfy the signature
(carrying the lattice's own failure), or a member could not take the view's type
for it. A refusal binds nothing and writes nothing but what a partial coercion
walk had already laid down, which no name reaches.

## Members are born coerced

A view's members take the view's types where the view is built, so every read
surface agrees by construction and nothing downstream re-checks. Under `:|` a
member declared at an abstract member no longer has the type it had in the
source, so it is not carried but rebuilt — [`coerce`](coerce.rs).

**The walk recurses on the declared type**, never on the two substituted types
in lockstep. A union interns its members in a canonical order, so the source's
substitution and the view's do not correspond position for position; only the
declaration lines them up. At each step the declared type read under `from` and
under `to` says what the member is coming from and going to, and **where the two
agree there is nothing to do** — which is the whole of `:!`, and every slot of
`:|` that names no abstract member.

Where they differ, the arm is the declared type's:

- a reference to an abstract member, first-order or applied — the value takes
  the mint as its one tagged layer, through
  [`sealing`](../../values/README.md#what-a-value-is), the second checked
  constructor beside `construction`: an abstract type records no representation
  to check a construction against, so what is checked is that the identity is a
  per-application mint and that the payload satisfies the source's own binding;
- a list, dict or record — rebuilt cell by cell through its own public door and
  re-stamped with the declared handle where the derived memo is not that one,
  which is the empty container and the widened declaration; a dict's keys are
  untouched, since a key type names no member a signature can declare abstract;
- a union — the first declared member whose source side admits the value, in the
  union's interned order, then that member's arm; two members that both admit it
  take whichever that order reaches first;
- a function type — a **barrier node**
  ([`Coerced`](../README.md#what-a-knot-member-is)) holding the
  function it stands before, the type a caller sees, the declared slot type and
  the two substitutions as signature handles. Teaching a call to go through it
  is [module programs](../../../roadmap/rewrite/modules.md)';
- a signature — the member is **re-viewed** through the same `build`. Nothing is
  minted at a nested boundary: the nested signature's slot types name the
  *enclosing* signature's members, so the enclosing substitutions read them and
  the nested view's abstract identities are the outer mints, arriving through
  the declared type rather than being made again.

Anything else, and any value whose shape does not match the arm its declaration
took, is a `CoercionRefused` naming what it was.

## Entering a `USING … SCOPE` block

The surfaced names are the block's **parameters**, read off the operand's
declaration where the shape is built
([names that arrive at run time](../../scope/README.md#names-that-arrive-at-run-time)),
so a mention of one resolves through the ordinary local read and a callable
nested in the block captures it the ordinary way. No coordinate names a member.

What is left at run time is the binding, and layout order is what makes it an
index: [`surface`](surface.rs) walks the block's slots, picks the parameters out
by declared position — a block declares locals of its own, and a local sorts in
among the parameters rather than after them — and binds the `k`-th parameter of
a channel to member `k` of that channel. The walk is staged first, so a refusal
binds nothing.

Its three refusals are unreachable from koan source: the block's parameters
*are* the module's declared names, so a block and the module it surfaces cannot
disagree. They guard a caller that hands the door the wrong module.

## The import rule

`knot`'s [boundary test](../tests/boundary.rs) covers this module with the rest
of the layer: outside doc comments and `#[cfg(test)]` it names no crate path but
the layers below, and holds no owning heap type. What that scanner cannot read is
the direction inside `knot`: **this module never names
[`function`](../function.rs)**, and reaches the node vocabulary through the
facade.

## Testing

The suites run a program through
[`knot`'s fixture](../README.md#testing) — the only thing that can
build a module to look at — which shapes and activates a program in a cell,
brings each binding into being, and for a module binder runs its body first and
ties the binder with the finished activation.

- [`tests/birth.rs`](tests/birth.rs) — a module node's signature and its members
  in layout order, a `GROUP` binder birthing one the same way, a module capturing
  an outer value and holding a module of its own, a body that ties a knot, two
  modules incomparable and rendering as their signature, and each refusal.
- [`tests/coerced.rs`](tests/coerced.rs) — what a barrier holds, and `values`
  seeing it as the function it stands for.
- [`tests/view.rs`](tests/view.rs) — a transparent view carries what the
  signature names and drops the rest, an opaque one mints a fresh carrier per
  application and sources it at its own nonce, each refusal, and a view copying
  across a cell like any module.
- [`tests/coerce.rs`](tests/coerce.rs) — one program per arm: every slot that
  names the carrier born at the mint, a function member behind a barrier, a
  nested module re-viewed at the outer mint, a signature-typed slot naming no
  member carried verbatim, a transparent view coercing nothing, and the two
  refusals.
- [`tests/surface.rs`](tests/surface.rs) — a surfaced name reading the member it
  names, the two refusals, and **the layout law** over three programs: the
  block, the body and the signature agree on every member's place.

The readings and refusals of the `USING` operand itself are
[`scope`'s](../../scope/README.md#names-that-arrive-at-run-time), in its example
suite, since they are facts about the shape.

## Open work

- [Module programs](../../../roadmap/rewrite/modules.md) — evaluating `:|`, `:!`
  and a member read as expressions, and calling a function member through its
  barrier.
- [Dispatch](../../../roadmap/rewrite/dispatch.md) — the keyworded channel of a
  module's signature, empty until a bodyless definition has a slot.
- [Unplanned work](../../../roadmap/rewrite/README.md#unplanned-work) — a cyclic
  data member coerced through a barrier, and `WITH` over a signature in a type
  expression.
