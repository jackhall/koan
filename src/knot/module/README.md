# Modules as values

The layer that reads a module by name: a member read through the signature a
module carries, the view `:|` builds, the coercion that births a view's
members, and the binding a `USING … SCOPE` block enters on.

A module is one kind of [knot](../README.md) node, holding its self-signature
and its members. This module owns that node — its payload, its doors and
everything that reads one by name — beside the [function](../function.rs) node
its sibling owns, which it never names. It reaches the shared node vocabulary
through [`knot`](../README.md#what-sits-where) and rests on
[`values`](../../values/README.md), [`scope`](../../scope/README.md),
[`elaborate`](../../elaborate/README.md) and the
[type lattice](../../../lattice/src/types/README.md).

What it does *not* do is evaluate anything.
[Dispatch](../../dispatch/README.md) evaluates `m :| Sig`, `m.f` and
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
digest does not name, a view's its signature application over the content of
its **underlying** — the module behind any header and any opaque view — and
`from*`, what *fits* solved each head parameter to over the underlying's
signature ([the view door](#the-view-door),
[carriers and paths](../../../design/modules.md#carriers-and-paths)). A view's
node also keeps a reference to its underlying and a **view record** — the
application it was built at and its two substitutions, as handles, and
`hidden`, each carrier its types show — one its `to` binds, or one its
application names in a pin or in its signature — beside the type it hides over
the underlying — so a view of it is built and keyed over the same underlying.
`hidden` is a function of the application and `to`, since a carrier is keyed on
the content of the view that made it. The copy and the exit walk
carry the reference as they carry a barrier's function, and a view's knot
weighs its underlying in, since a crossing copies it. Every module is a view:
a body-born module, and a module a barrier re-views inward
([below](#members-are-born-coerced)), is a transparent view of itself at the
signature it is born at, hiding nothing and its own underlying, so it keeps no
record and no reference.
`ModuleContent` ([module.rs](../module.rs)) owns that recipe
— one constructor per kind, and `Module::tie` takes nothing else — so a
module's content is computed one way wherever one is tied. It is computed at
the first view or key that reaches the module and kept on the module behind a
pointer, so the node keeps its width and a module nothing views pays for no
digest. A body-born module holds its body's closure for life, as a function
does, and its copy and the exit walk carry it, so a copy computes the content
its source would. A `:|` over a module binder read bare, in the activation
the module was born in or a block inside it, fills the content from the
digests the captured slots keep, which the shape reads off the view
([memos](../../scope/shapes/memos.rs), `view::fill_at_view`).

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
each overload its underlying offers there that the member admits. No signature
table indexes the tail: the module's signature carries each registered shape in
its keyworded channel, and a reader finds a key's functions by their registered
shapes, never by position (`layout::functions_at`).

So `m.f` is an index rather than a search. The sort is by interned symbol — a
content digest — not by the text of a name, so nothing may read the order as
alphabetical.

[`layout`](layout.rs) holds the readers of a module value: `member`, what `m.f`
reads, `registrations` and `functions_at`.

**A module is read through the signature it carries.** A module retyped to a
signature type sits under a
[header](../../values/README.md#the-type-memo-and-satisfies) carrying the
application it fits, each head parameter pinned at what *fits* solves, and a
bare module carries its own signature. `member` takes its declared type from
that signature and its index from the node's own, and hands the member back
retyped to its declared type — a function member as a retyped function, a
quantified one at its declared scheme — so a field the signature hides is no
field of it. A type member is a type value of its declared type, which the node
need not bind: a nested module declared at `Inner WITH {Elem = Number}` reads
`Elem` as `Number`. Under a meet, each member is read at the meet of what the
applications declaring it declare.

**A keyworded member offers its pick.** Through a header, `functions_at` offers
at a key, per keyworded member the carried signature declares there, the
module's overloads **covering** it — each whose registered shape satisfies the
member's — narrowed by the selection's own ranking to those ranking first among
themselves (`covering`), each retyped to the member's shape. Every overload
covering a member admits every call the member admits, and their ranking does
not depend on the arguments, so the pick made once where the module is read is
the one every call would make: a call the declared shape refuses selects
nothing, and two covering overloads with neither ranking first are both
offered, and ambiguous where a call meets them.

## The view door

[`ascribe`](view.rs) is what `m :| Sig` builds; `m :! Sig` is a
[retype](#layout-order), which re-stamps the module under a header and never
comes here. A view is a module of its own — the same node, through the same
constructor a body-born one goes through — holding only the members its
signature names, at the types that signature declares, built over its
underlying's members. So a view and a
body-born module have one representation and one copy, and nothing downstream
asks which it holds. A keyworded member is carried as the pick a header makes
([above](#layout-order)) among the underlying's overloads at its key, read under
`from*` — each behind a barrier where the view reads the member
otherwise, and retyped to the member's shape the view shows.

Only `:|` is a view. The run and
[the load](../../dispatch/README.md#static-types) both read the operator
(`Ascription::Opaque`), so the load refuses what every run's door refuses.

The ascribed type must be one application of a declared signature (`takes`),
`Counter` or `Counter WITH {Carrier = Number}`; a meet of several is refused,
since a view lays out one signature's members. The source's signature is the
type it carries — a header's application, or a bare module's own — and the
door checks, through the solve a retype pins by (`values::fit_solution`), that
it
[*fits*](../../../lattice/src/types/relations.md#signature-types) the
application, which solves each head parameter the application leaves unpinned
from what the source's members offer, and then fixes two substitutions over the
signature's head parameters:

- `from*` — what *fits* solves each one to over the **underlying's**
  signature, or its pin: under
  `SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]`, an underlying
  binding `zero` to `0` gives `Carrier` the type `Number`, whether the source
  is that module, a view of it or a retype of either. Where the source is a
  view, the ascribed application is read back onto the underlying before the
  fit: each of the source's carriers it holds, anywhere in a pin or in the
  signature, under a signature node too, replaced by what the source's
  `hidden` says it hides (the lattice's `substitute_carriers`). So
  `(v :| (Paired WITH {Elem = (LIST OF v.Carrier)}))`, `v` a view of `nums`
  hiding `Number` behind `v.Carrier`, solves `Elem` to `LIST OF Number` over
  `nums`, and `(v :| Named)`, `Named` declaring `zero` at `v.Carrier`, fits
  over `nums` as `Named` at `Number` does. *Fits* is transitive, so a fit the
  source admitted holds over the underlying;
- `to` — a **carrier per unpinned parameter**, keyed on the view's own content.
  A carrier records the declaration's bound as the bound it met, which only the
  signature's fit reads, so the view fits what its source did; anywhere else it
  lies under `Any` alone, so a member sealed under it is opaque outside the view
  whatever its bound. A pinned parameter keeps its pin.

Everything after that is one fit, `fitted`, and one body, [`build`](view.rs),
which the nested case below goes through too.

**A view of a view** is built over its source's underlying, or refused. The
door checks, from types alone (`hides_again`), that every part the source
seals sits, in the new view, at a part whose view-side type is a carrier: a
head parameter the new view leaves unpinned, or a carrier a pin holds,
anywhere in the pin. The check walks each member's declared type beside the
source's, by the coercion walk's arms, a bare signature pairing as the
application of it that pins nothing, reading the source's underlying side
through its `hidden` and a source parameter through what the source binds it
to; a union pairs its members by type handle, never by position, and a part
the source sealed whole sits at the new union's member that reads as it; a
keyworded member is checked against the source members covering it. Where the
check holds, the view is built over the underlying exactly as a view of it
would be: one seal and one barrier, nothing of the source's carried. Where it
fails, the view is refused (`Reveals`), since built over the underlying it
would unseal what the source hides, and built over the source a chain of
views alternating two signatures would never repeat. Where the computed
content equals the source's own, the door returns the source.

The carrier is a [`Carrier` node](../../../lattice/src/types/vocabulary.md)
keyed on the view's content — its application over its underlying's content
and `from*`, the one `ModuleContent` the view is tied with — and the
parameter's name keeps two parameters of one view apart. The source as
written, header included, decides only whether the view is admitted, and the
members are built over the underlying, so a view is what it shows of the
module it hides. `ints :| Counter` and `m :| Counter`, `m` being `ints` passed
through a parameter `m :Counter`, are one view with one carrier, and so is
`(ints :! (Counter WITH {Carrier = Any})) :| Counter`.
`(ints :| Counter) :| Counter` is that view, returned. `(ints :| Counter) :|
Small`, where *fits* admits `Small` over the view and `Small` hides again what
`Counter` hides, is `ints :| Small`. Two ascriptions of one signature over modules of equal content therefore
produce views with one carrier, and over modules that differ views whose
carriers do not unify.

The view carries a signature of its own — each head parameter a manifest member
at what `to` gives it, every manifest member, value slot and keyworded member at
its declared type read under `to` — so a view's signature is a module's, with no
parameters, and it fits the signature it was ascribed to.

**The type half.** Everything the door computes from types alone — the
schema, the admission fit, the check, both substitutions, the content, the view
record and the view's signature — is [`exposure`](view.rs), over an operand
known by its carried type, its bare member's content, and that member as a
view: its own signature and, for an opaque view, its record and its
underlying's signature and digest. A transparent source hides nothing, so the
check holds over it and it is its own underlying: the door has one path.
Where the load does not know the bare member as a view, or does not know the
carried type, the view is open; a fully pinned application over a source known
to hide nothing still solves its pins. A frame binding a module records it as a
view beside its digest
([activations](../../build/README.md#activations)), so a view over a module
parameter is settled for each binding. [`nested_types`](view.rs) is the same half of an
inward re-view (below). A nested view at a slot whose application pins every
parameter needs no operand: *fits* solves a fully pinned application to its
pins over any module, so [`pinned_view`](view.rs) computes its `from*`, `to`,
signature and `hidden` from the slot's two sides alone. The load
calls these at a `:|` site over a content tree it sees all of, and interns what
the walk would — each member's declared type under either side and each nested
module's view — so the run's view finds every one of those types already in
the root ([closed views at load](../../dispatch/README.md#paths)).

An `Unascribable` names which door refused: the operand is no module, the
ascribed handle names no one application of a signature, the module does not fit it
(carrying the lattice's own failure), a view of a view shows a part its source
hides (naming the member), or a member could not take the view's type for it.
A refusal binds nothing and writes nothing but what a partial
coercion walk had already laid down, which no name reaches.

## Members are born coerced

A view's members take the view's types where the view is built, so every read
surface agrees by construction and nothing downstream re-checks. Under `:|` a
member declared at an unpinned head parameter no longer has the type it had in
the source, so it is not carried but rebuilt — [`coerce`](coerce.rs).

**The walk recurses on the declared type**, never on the two substituted types
in lockstep. A union interns its members in a canonical order, so the source's
substitution and the view's do not correspond position for position; only the
declaration lines them up. At each step the declared type read under `from` and
under `to` — on the underlying's side, each of the view's carriers read as what
it hides — says what the member is coming from and going to, and **where the two
agree there is nothing to do** — every slot that names no unpinned parameter. Where they differ and a side still names
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
  carrier and that the value satisfies the source side, the underlying's own
  binding. A view is built over its underlying, so the source side is never a
  carrier of the view's source. It is a carrier only where the underlying
  binds a value another view sealed, as `MODULE n = ((LET zero = c.zero))`
  binds one of `c`'s, and the value is then sealed again under the view's
  carrier. Where only the
  source side is a carrier — a value crossing a barrier inward — a value
  sealed under it gives up its payload at the view's type. Where neither side
  is — a parameter pinned at a type holding the view's carriers — the value is
  rebuilt along the pin, read as written on the view's side and through
  `hidden` on the underlying's, the walk meeting each carrier the pin holds as
  a parameter's arm;
- a list, dict or record value — rebuilt part by part from what its type shows,
  a record value from the fields its slot declares and no other, and laid
  straight at the slot's type read under `to` (`List::typed`, `Dict::typed`,
  `Record::typed`), which each rebuilt part satisfies, so the walk interns no
  type of its own for the container; a dict's keys are
  untouched, so a key read back through the view carries the source's scalar
  type even where the dict's declared key type names a carrier;
- a union — the member whose source side admits the value, by a rule blind to
  the union's member order: where several admit it, the one naming a parameter
  the view hides is taken, and two such refuse (`TiedUnion`); a member naming
  none reads alike either side, so any one of those carries the value as it is.
  Then that member's arm;
- a function type or an expression shape — a **barrier node**
  ([`Coerced`](../README.md#what-a-knot-member-is)) holding the
  function it stands before, the type a caller sees, the declared slot type,
  the two substitutions as signature handles, which `exposure` interns once
  per view and a call through the barrier reads as the handles' manifest
  members, and the view's `hidden`, which a crossing reads on the underlying's
  side, inward with the sides swapped;
- an application whose pins name the enclosing signature's parameters, or a
  signature naming an enclosing carrier, the application of it that pins
  nothing — outward, the underlying's nested member is **viewed** through the same
  `exposure` and `build` as `:|`, at the slot's type read under `to`, its
  admission already decided by the enclosing fit and its `from*` solved at the
  slot's type read under `from*`: the enclosing carriers arrive through the
  pins, and each parameter of the nested signature the application leaves
  unpinned gets a carrier keyed on the nested view's content. A module a
  barrier's function returns crosses outward the same way, its admission
  checked here, since no enclosing fit decided it. Inward, a nested
  module is **re-viewed** transparently through `fitted` and `build`, its
  types those [`nested_types`](view.rs) computes: its members built over the
  slot's type read on the barrier's outer side, and its own type laid from it
  read on the function's, so a bare signature's re-view takes the signature
  read back through the carriers it names. Its content is tagged apart
  (`Operator::Reviewed`); the re-view is a transparent view of itself, its
  own underlying, so a view of it inside the function is built over it. A
  signature-typed slot whose type names no enclosing parameter or carrier
  reads alike either side, and is carried.

Anything else, and any value whose shape does not match the arm its declaration
took, is a `CoercionRefused` naming what it was.

**A call through a barrier** crosses it both ways. Its arguments cross
[`inward`](coerce.rs): the same walk with the two substitutions swapped, where a
parameter's arm unseals a value sealed under the carrier and refuses one that
is not (`NotSealed`). A function member's parameters are crossed by name, and a
keyworded member's slots under the names the function behind the barrier
registers. The function behind it is then called by name, so its own frame
admits the arguments and solves its group. Its result crosses back
[`outward`](coerce.rs) at the declared return. A call through a barrier never
tails, since its value crosses where its frame ends.

A quantified member behind a barrier, read where the load solved its group — an
[instance site](../../dispatch/README.md#static-types) — is a barrier of its own
at the instance, [`barrier_instance`](../function.rs): the instance is what a
caller sees and what its arguments cross at, and the same function stands behind
it, solving its own group where it runs.

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
- [`tests/view.rs`](tests/view.rs) — a retype at a signature reads what the
  signature names at what *fits* solves under a header over the module's own
  node, an opaque view keys its carrier on content under the parameter's name —
  one content whatever header its source is under — records its bound and
  reveals it nowhere, and keeps a pin, a view of a view built over its
  underlying or refused, a view
  carries the overload each keyworded member picks and wraps one over a carrier
  in a barrier, each refusal, and a view copying across a cell like any module.
- [`tests/coerce.rs`](tests/coerce.rs) — one program per arm: every slot that
  names the carrier born at it, a function member behind a barrier, a nested
  module viewed with a carrier for each parameter its slot leaves unpinned and
  re-viewed transparently crossing inward, a signature-typed slot naming no member
  carried verbatim, a union member naming
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

- [What a barrier rebuilds](../../../roadmap/types/barrier-coercion.md)
  — a cyclic data member and a dict's keys, each coerced through a barrier.
