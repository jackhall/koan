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
[type lattice](../../../lattice/src/types/README.md).

What it does *not* do is evaluate anything.
[Dispatch](../../dispatch/README.md) evaluates `m :| Sig`, `m :! Sig`, `m.f` and
a `USING` expression through the doors here, and a
[frame](../../program/README.md) called through a barrier crosses it with the
coercion walk below. What a module's types are, and when two are one, is the
[module design](../../../design/modules.md)'s.

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

A module's node also holds its **content**, the digest its knot's
[digest](../../values/README.md#content-digests) covers it by. A body-born
module's is its body's code digest over the digest of each capture the code
digest does not name, a view's its operator and signature application over its
source's digest. `ModuleContent` ([module.rs](../module.rs)) owns that recipe
— one constructor per kind, and `Module::tie` takes nothing else — so a
module's content is computed one way wherever one is tied. A module keeps no
captures to digest later, so its content is computed where it is tied, and it
sits behind a pointer so the node keeps its width.

## Layout order

Every door here rests on one rule, which
[`elaborate`'s signature members](../../elaborate/README.md#a-signatures-members)
spell: value members, sorted by name, then type members, sorted by name. **The
signature alone places a named member**, through the one lookup the type reader
and dispatch read too:

- a body-born module's **tie** places each slot its body binds by name, at the
  index the module's own signature gives that name, so nothing assumes the
  body's slot order;
- a **view**'s member run is built by walking its signature's tables, which are
  symbol-sorted by name;
- a **`USING` block** binds each surfaced parameter by name, at the index the
  module's signature gives it.

A module's run then goes on past its named members with the functions it
offers at a key. A body-born module holds each registration its body declares,
in the shape's registration order — the function born for a bare `EXPR` or `OP`
statement, or for a combined one's bucket beside its named value slot. A view
holds, for each keyworded member of its signature in the signature's order,
each overload the source offers there that the member admits. No signature
table indexes the tail: the module's signature carries each registered shape in
its keyworded channel, and a reader finds a key's functions by their registered
shapes, never by position (`layout::functions_at`).

So `m.f` is an index rather than a search. The sort is by interned symbol — a
content digest — not by the text of a name, so nothing may read the order as
alphabetical.

[`layout`](layout.rs) holds the readers of a module value: `member`, what `m.f`
reads — the member's index in the signature, then that index into the node's
run — `registrations` and `functions_at`.

## The view door

[`ascribe`](view.rs) is what `m :! Sig` and `m :| Sig` build. A view is a module
of its own — the same node, through the same constructor a body-born one goes
through — holding only the members its signature names, at the types that
signature declares. So a view and a body-born module have one representation and
one copy, and nothing downstream asks which it holds. A keyworded member is
carried as the source's overloads at its key that the member, read under the
source's bindings, admits — each behind a barrier where the view reads the
member otherwise.

Which ascriptions are views is one predicate, `views`: `:|` always, and `:!`
where the operand's type and the ascribed type are both signature types. The
run reads it over the carried type and [the load](../../dispatch/README.md#static-types)
over the static one, so the load refuses what every run's door refuses.

The ascribed type must be one application of a declared signature (`takes`),
`Counter` or `Counter WITH {Carrier = Number}`; a meet of several is refused,
since a view lays out one signature's members. The door checks that the source's
self-signature [*fits*](../../../lattice/src/types/relations.md#signature-types) the
application, which solves each head parameter the application leaves unpinned
from what the source's members offer, and then fixes two substitutions over the
signature's head parameters:

- `from` — what *fits* solved each one to, or its pin: under
  `SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]`, a source binding
  `zero` to `0` gives `Carrier` the type `Number`;
- `to` — under `:!`, `from` itself; under `:|`, a **carrier per unpinned
  parameter**, keyed on the view's own content. A carrier records the
  declaration's bound as the bound it met, which only the signature's fit reads,
  so the view fits what its source did; anywhere else it lies under `Any` alone,
  so a member sealed under it is opaque outside the view whatever its bound. A
  pinned parameter keeps its pin either way.

Everything after that is one fit, `fitted`, and one body, [`build`](view.rs),
which the nested case below goes through too, so the two operators are not two
paths that agree: `:!` is the case of `:|` where `to` is `from`, and
every coercion below stops at its first comparison. Two `:|` ascriptions of one
signature over modules of equal content therefore produce views with one
carrier, over modules that differ views whose carriers do not unify, while `:!`
is a relabelling.

The carrier is a [`Carrier` node](../../../lattice/src/types/vocabulary.md)
keyed on the view's content — its operator and application over the source's
digest, the one `ModuleContent` the view is tied with — and the parameter's
name keeps two parameters of one view apart.

The view carries a signature of its own — each head parameter a manifest member
at what `to` gives it, every manifest member, value slot and keyworded member at
its declared type read under `to` — so a view's signature is a module's, with no
parameters, and it fits the signature it was ascribed to.

An `Unascribable` names which door refused: the operand is no module, the
ascribed handle names no one application of a signature, the module does not fit it
(carrying the lattice's own failure), or a member could not take the view's type
for it. A refusal binds nothing and writes nothing but what a partial coercion
walk had already laid down, which no name reaches.

## Members are born coerced

A view's members take the view's types where the view is built, so every read
surface agrees by construction and nothing downstream re-checks. Under `:|` a
member declared at an unpinned head parameter no longer has the type it had in
the source, so it is not carried but rebuilt — [`coerce`](coerce.rs).

**The walk recurses on the declared type**, never on the two substituted types
in lockstep. A union interns its members in a canonical order, so the source's
substitution and the view's do not correspond position for position; only the
declaration lines them up. At each step the declared type read under `from` and
under `to` says what the member is coming from and going to, and **where the two
agree there is nothing to do** — which is the whole of `:!`, and every slot of
`:|` that names no unpinned parameter. Where they differ and a side still names
a variable — a slot over the member's own `FOR ALL` group — there is no type to
rebuild the value at, and it is refused. A barrier's
arguments and result, and a view's keyworded members, go through this one
walk.

Where they differ, the arm is the declared type's:

- a reference to a head parameter — where the view side is a carrier, the
  value takes it as its one tagged layer, through
  [`sealing`](../../values/README.md#what-a-value-is), the second checked
  constructor beside `construction`: a parameter records no representation to
  check a construction against, so what is checked is that the identity is a
  carrier and that the value satisfies the source side — the source's own
  binding, or, for a view of an opaque view, the inner view's carrier, so the
  value is resealed and the two views' barriers stack. Where only the source
  side is a carrier, a value sealed under it gives up its payload at the view's
  type;
- a list, dict or record — rebuilt part by part from what its type shows, a
  record from the fields its slot declares and no other, through its own public
  door and re-stamped with the declared handle where the derived memo is not that one,
  which is the empty container and the widened declaration; a dict's keys are
  untouched, so a key read back through the view carries the source's scalar
  type even where the dict's declared key type names a carrier;
- a union — the member whose source side admits the value, by a rule blind to
  the union's member order: where several admit it, the one naming a parameter
  the view hides is taken, and two such refuse (`TiedUnion`); a member naming
  none reads alike either side, so any one of those carries the value as it is.
  Then that member's arm;
- a function type or an expression shape — a **barrier node**
  ([`Coerced`](../README.md#what-a-knot-member-is)) holding the
  function it stands before, the type a caller sees, the declared slot type and
  the two substitutions as signature handles;
- an application whose pins name the enclosing signature's parameters — the
  member is **re-viewed** through the same `fitted` and `build`. No carrier is made at a
  nested boundary: the enclosing substitutions read the pins, so the nested
  view's identities are the outer carriers, arriving through the declared type
  rather than being made again, and the nested signature's own unpinned
  parameters keep what *fits* solves them to. A signature-typed slot whose type
  names no enclosing parameter reads alike either side, and is carried.

Anything else, and any value whose shape does not match the arm its declaration
took, is a `CoercionRefused` naming what it was.

**A call through a barrier** crosses it both ways. Its arguments cross
[`inward`](coerce.rs): the same walk with the two substitutions swapped, where a
parameter's arm unseals a value sealed under the carrier and refuses one that
is not (`NotSealed`), or, at a barrier stacked before another view's, reseals
it under that view's carrier. A function member's parameters are crossed by name, and a
keyworded member's slots under the names the function behind the barrier
registers. The function behind it is then called by name, so its own frame
admits the arguments and solves its group. Its result crosses back
[`outward`](coerce.rs) at the declared return. A barrier stacked behind another
is crossed in turn, and a call through one never tails, since its value crosses
where its frame ends.

## Entering a `USING … SCOPE` block

The surfaced names are the block's **parameters**, read off the operand's
declaration where the shape is built
([names that arrive at run time](../../scope/README.md#names-that-arrive-at-run-time)),
so a mention of one resolves through the ordinary local read and a callable
nested in the block captures it the ordinary way. No coordinate names a member.

What is left at run time is the binding: [`surface`](surface.rs) walks the
block's slots, picks the parameters out by declared position — a block declares
locals of its own, and a local sorts in among the parameters rather than after
them — and binds each parameter to the member of its name, at the index the
module's signature gives it. A surfaced key is a registration
parameter, bound to the list of every function the module offers at the key
(`layout::functions_at`), which a use at the key spreads. The walk is staged
first, so a refusal binds nothing.

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
  signature names at what *fits* solves, an opaque one keys its carrier on
  content under the parameter's name, records its bound and reveals it nowhere,
  and keeps a pin, a view carries
  each overload its keyworded members admit and wraps one over a carrier in a
  barrier, each refusal, and a view copying across a cell like any module.
- [`tests/coerce.rs`](tests/coerce.rs) — one program per arm: every slot that
  names the carrier born at it, a function member behind a barrier, a nested
  module re-viewed at the outer carrier, a signature-typed slot naming no member
  carried verbatim, a transparent view coercing nothing, a union member naming
  a hidden parameter taking the value whatever the union's order, and the
  refusals.
- [`tests/surface.rs`](tests/surface.rs) — a surfaced name reading the member it
  names, the two refusals, and every member read by its name over four
  programs, one whose names intern out of their written order: the module's
  member and the block's parameter of each name hold the value the body bound.

The readings and refusals of the `USING` operand itself are
[`scope`'s](../../scope/README.md#names-that-arrive-at-run-time), in its example
suite, since they are facts about the shape. Programs that ascribe, read members,
open a module and call through a barrier are
[dispatch's](../../dispatch/README.md#testing) module suite.

## Open work

- [Path types](../../../roadmap/rewrite/path-types.md) — a type member named at
  load through a module-valued expression.
- [Unplanned work](../../../roadmap/rewrite/README.md#unplanned-work) — a cyclic
  data member coerced through a barrier, and a dict's keys crossing a barrier
  unsealed.
