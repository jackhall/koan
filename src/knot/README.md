# Knots

Functions, modules and circular data as values: the layer that closes
[`Value`](../values.rs)'s knot-member parameter with a node that is a function,
a data node, a module or a barrier over a function, ties a component of value
binders as one [knot](../memory/README.md#the-knot), and copies a member by
re-tying its knot.
It sits above [`values`](../values/README.md), [`scope`](../scope/README.md)
and [`elaborate`](../elaborate/README.md), and neither `values` nor `scope`
names it. It is the top of the rewrite's value stack.

## What sits where

[`knot.rs`](../knot.rs) is the vocabulary every node kind shares: the member,
the node, the value and activation spelled at it, and the `Supplied` and
`Untieable` a birth answers with. Each node kind owns its payload and its doors
beside it:

- [`function.rs`](function.rs) — a function node, and the staging a tie does for
  a function member.
- [`data.rs`](data.rs) — a data node: staging, the memo derivation below, and
  the lay-down.
- [`module/`](module/README.md) — a module node and a barrier node, and
  everything that reads a module by name.

[`tie.rs`](tie.rs) births a component over them, and [`copy.rs`](copy.rs) re-ties
a whole knot. A node kind reaches the shared vocabulary through the facade and
never a sibling, so `function` and `module` do not name each other.

## What a knot member is

`values` does not define a knot member. Its `Value<'graph, 'cell, X>` has one
arm, `Knotted(X)`, over a parameter it asks two things of
([What a value is](../values/README.md#what-a-value-is)), and `scope` threads
the same parameter through closure bindings, the builtin table and the
activation. This module closes it:

- [`Knotted`](../knot.rs) is one member of a knot — the `(knot, index)`
  pair, sixteen bytes — so a value holding one stays a twenty-four-byte word.
  Its equality is node identity.
- The knot's payload is a `Node`, of four arms. A `Function` node holds the
  function's memoized type handle, the body shape it runs (in program storage),
  its closure bindings, and the weight of the whole knot it sits in. A `Data`
  node holds a [`Circular`](../values/circular.rs) — a list, dict, record or
  tagged resident whose cells are links — and the same knot weight. A `Module`
  node holds its self-signature, its members in
  [layout order](module/README.md#layout-order) and the same knot weight —
  not an activation, since a view has no body to activate and a shape built per
  application would grow program storage without bound. A `Coerced` node is a
  **barrier** over a function member of an opaque view
  ([members are born coerced](module/README.md#members-are-born-coerced)):
  the function it stands before, the type a caller sees, the slot type the
  view's signature declares, and the two substitutions it coerces between.
  `values::Knotted::resolve` tells a data node from the rest, and
  `Knotted::function`, `::module` and `::coerced` each answer only for their own
  arm.
- `KValue`, `KValueFamily`, `KValueCarrier` and `KActivation` spell the value,
  its family, its carrier and the activation at this parameter.

**A node is sixty-four bytes**, pinned by a `const` assertion, because every
node in the program pays for the widest arm. The module arm sets that width; a
barrier's six fields would widen every node, so `Coerced` points at a resident
struct beside the node instead, at the price of one pointer hop to read a
barrier and one extra resident write per barrier born. The next arm has to
justify itself against the same pin.

Every function is a knot node, and so is every module. A function that names no
fellow is a one-node knot, and a module always is: a mention reached from a
module binder's root is eager whatever body it sits in, so a module is never in
a cycle — with a fellow binder, which the shape refuses as an eager cycle, nor
with itself, which is refused a step earlier, since an eager read at the
binder's own position does not see that binder. So there is one representation,
one birth path and one copy per kind of node. A node is region-resident, `Copy`
and so `Drop`-free, and a node's fields are private to this module: a node
exists only because the tie, the view door one layer up, or a copy of a tied
knot, laid it down.

## The tie

[`tie`](tie.rs) births one deferred-only component of a body's bindings
([Visibility](../scope/README.md#visibility)) as one knot in the writer's
region, and hands the knot back. **The caller binds**: member `i` of the
component is the knot's node `i`, and the caller binds each member's slot to
`Knotted::of(knot, i)`. A component is therefore one unit of work that binds
several slots. The caller ties a component of value binders when it is cyclic
(`Component::cyclic`) or when every member births a callable; a lone data
binder that reads nothing of its own is an ordinary value.

**Members.** A member is a *function member* when its binder births a callable:
its right-hand side is a callable form at its root, through transparent groups
(`LET f = (FN …)`), or its form is a combined one (`LET f = FN EXPR …`,
`LET f = OP …`). It is a *data member* when it is a `LET` whose right-hand side
(`Shape::rhs`), through one-part groups, is a list, dict or record literal or a
nominal construction `(Head payload)`. It is a *module member* when it is a
`MODULE` or `GROUP` binder — a member class of its own rather than `Opaque`.
Anything else is `Opaque` and refuses the tie before a member is read. A
callable form anywhere else — inside a list, or called where it is written — is
not a binder's and is born by no tie.

**A module member is born body-first, and alone**, on the module layer's own
path ([Birth](module/README.md#birth)): its binder's body has already run in an
activation the caller built through `body_activation` — a module shape's
activation, which carries no callable, because a module's captures are never
edges — and hands to the tie by site, with every slot bound. `tie_member` asks
[`elaborate`](../elaborate/README.md#a-modules-self-signature) for the
self-signature over that activation and reads its slots out in slot order, which
is [layout order](module/README.md#layout-order), so a module's type and weight
are facts about the members its body bound and the node is written once. A slot
the body left claimed refuses `Pending` on that binder; no body supplied refuses
`Eager` at the body's own site. Because a module is alone in its component, it
shares nothing with the staging below.

**Stage, with no writer in reach.** Everything a member needs is read into
scratch first.

- A function member: the body shape it births (`Shape::births`), the
  function's type, elaborated from the form node its body sits in
  ([A callable's type](../elaborate/README.md#a-callables-type)), and its
  captures, read from the enclosing activation
  (`ClosureBindings::read_captures`).
- A data member ([data.rs](data.rs)): its right-hand side walked part by part.
  A literal waits to be lowered; a mention of a fellow member is an edge; any
  other mention is the word the activation reads there. A part the walk cannot
  build itself — a call, a keyword form, a `FN` — is asked of the caller's
  evaluator by its `Site`, so the caller evaluates the eager parts and ties
  again with their values; a dict key is a scalar literal or such a part.
- **A deferred mention below a nested constructor is written into the knot.**
  Every constructor on the path from a member's root to a fellow mention is an
  *anonymous* data node of the same knot, indexed after the members in the
  order the walk meets it, and the cell that held it holds an edge to it. So in
  `LET a = {inner = [f], plain = [1 2]}` with `f` in `a`'s component, `inner`
  is an edge to an anonymous list node whose cell is an edge to `f`, while
  `plain`, which names no fellow, stays an ordinary value.

Every mention of a fellow member becomes the edge the knot's plan mints for
that node's index.

**Derive memos: the nominal cut.** A function node's memo is its signature
type and a tagged node's is the newtype its head names; both are declared, and
exist before the knot does. A container node's memo is derived by the rule the plain door of
its kind uses (`values::list_type`, `dict_type`, `record_type`) — the join of its cells for a list, the key and value
joins for a dict, the record type of its fields — with an edge contributing
its target's memo. So the tie derives container memos over the strongly
connected components of the edges between container nodes, referents first,
reading a declared memo at every cut. A cycle of container nodes alone has no
finite type and refuses the tie as `TypeCycle`, naming the members whose
right-hand sides hold it. An ascription is no cut, since a retype stamps a
structural type and no structural type names itself: `LET a = [1 a]` refuses,
while `LET a = (Ring {next = a})` over `NEWTYPE Ring = :{next :Ring}` ties as a
tagged node over an anonymous record node `{next: Ring}`, and `LET a = [f]`
with `f` a function capturing `a` ties with `a : List` of `f`'s type.

**Check constructions.** Every construction a data member holds is checked by
[`values::construction`](../values/README.md#what-a-value-is), the one rule an
ordinary construction goes through too: a tagged node against its derived
payload memo, and a construction that is no node against its staged payload's
type, so its later `Tagged::construct` cannot fail.

**Write.** Only then are the closure runs and each data node's resident laid
down, the knot's weight summed, and the nodes tied: member `i` is node `i`,
and the anonymous nodes follow.

Because nothing is written before every read and check has finished, **a
refusal writes nothing**. The refusal is an `Untieable`:

- `Opaque` — a member that is none of a function, module or data member.
- `Pending` — a read whose binder is still running — a capture, a type name in
  a signature, a data member's mention, a construction's head — with that
  binder's cell handle. The caller waits on the binder and ties again.
- `Eager` — a part of a data member at a site the evaluator has no value for,
  or a module member whose body the caller has not run. The caller supplies it
  and ties again; what it supplies is a value or a run body, the two arms of
  `Supplied`.
- `Key` — a dict key in a data member that evaluated to something no key can
  be.
- `Construction` — a construction the rule refuses, with its site.
- `TypeCycle` — a cycle of container nodes, with the members holding it.
- `Type` — a member's signature did not elaborate.

A `FN` under a data member's constructor slot is an eager part no evaluator can
supply yet — a function value born outside a binder's root has no birth path —
so it refuses `Eager`.

## Closure bindings and edges

A function's captured environment is the scope layer's
[closure bindings](../scope/README.md#three-tiers): one run, in capture-slot
order, of [links](../values/README.md#what-a-value-is) — value words and knot
edges, the same cell type a data node holds. A value word is the enclosing
binding's word, shallow, so capturing a string shares its bytes. An edge names
a fellow node by index.

No reader sees a bare edge. An activation of a function holds the member it
runs, and a read of a capture that is an edge resolves through it
(`Knotted::sibling`) to the sibling member — a function or a data node. A data
node's links resolve through the member holding them the same way. A block
inside a function's body inherits its member, so a nested function capturing
an enclosing member reads it as that sibling's value word, and captures a value,
not an edge.

## Weight and copy

A member's weight is its knot's: the run header, one `Node` per node, each
function's closure run, each data node's resident, each module's member run and
each barrier's resident, with everything their value words point at. A member
that is itself a knot member contributes its *whole* knot's weight, since a
crossing rebuilds that knot whole. It is summed at the tie and memoized on every node, so a
crossing prices a member by reading one field, under the ordinary
[verdict](../values/README.md#crossing).

**Every knot member copies.** The value family's deep copy hands a member to
this module's family together with the copy itself, and
[`copy_into`](copy.rs) re-ties the whole knot in the destination's region:
every node rebuilt in index order — a function's closure run through
`ClosureBindings::copied`, a data node through `Circular::copied`, a module's
member run and a barrier's underlying function through that same copy — each held
value deep-copied through the copy the crossing priced, each edge carried
verbatim, since an edge names a node by index and so means the same node in
the copy, anonymous nodes included, and the types, body shapes and knot weight
carried over. The copied member is the one at the source's own index. A copy
never shares a node with its source, so a copied knot outlives the region it
was copied from.

A module's members are rebuilt through that one copy, so a member that is itself
a knot member brings its whole knot with it — and two members of one foreign
knot arrive as two copies of that knot, the price of "a member brings its knot",
which a data node holding two such words already pays. A barrier's underlying
function goes the same way, as the value word a member holding it would be.

## Equality and rendering

A function has no structural equality: any comparison that reaches one is
`Incomparable` ([Equality](../values/README.md#equality-and-rendering)), which
the `==` builtin reports rather than answering `false`. A function renders as
its type's name does, `:(FN :{x :Number} -> Number)`; its closure bindings are
program state and never print. A module and a barrier are opaque to `values` the
same way — a module renders as its signature's name, and a barrier as the
function type a caller sees. A data node compares as a bisimulation and
renders with `@n` labels where a cycle closes, both in `values`.

## The import rule

**Outside doc comments and `#[cfg(test)]`, `knot` names `crate::elaborate`,
`crate::memory`, `crate::parse`, `crate::scope`, `crate::symbols`,
`crate::type_lattice` and `crate::values`, and nothing else in the crate** — no
scheduler, no builtins.
[`tests::boundary`](tests/boundary.rs) reads this module's own source and fails
on any other `crate::` path, on an owning heap type outside the tests, and on a
retired lifetime name. `values`' and `scope`'s own boundary tests hold them
below this module, and nothing above names it but the scheduler.

The direction *inside* `knot` is spelled in `super`-relative paths, which that
scanner does not read: a node kind names the facade and never a sibling. It is
held by review.

## Testing

The suites run programs through a fixture that activates a program in a cell
and brings each binding into being — a component of type binders through
[the declaration door](../elaborate/README.md#declarations), a cyclic component
of value binders or a component of callable binders through the tie with an
evaluator that supplies nothing, a module binder body-first (its body's
activation laid down, every component of that body brought in, then the binder
tied with the finished activation), and a lone data binder whose right-hand side
lowers. The module suites read the same fixture, since it is the only thing that
can build a module to look at. A test that constructs a nominal writes its `NEWTYPE` in the program
source and reads the handle back off the slot the door bound; the builtin table
carries the scalar types alone.

- [`tests/birth.rs`](tests/birth.rs) — a lone function, a captured value word,
  mutual and self recursion read back through their edges, a nested capture of
  an enclosing edge; a tagged self-reference and a two-member tagged ring, a
  container sharing a knot with the function that captures it, an anonymous
  node on a sibling path, a nested construction built through the checked
  door, an eager part supplied by site; and each refusal.
- [`module/tests/birth.rs`](module/tests/birth.rs) — a module node's signature
  and its members in layout order, a `GROUP` binder birthing one the same way, a module
  capturing an outer value and holding a module of its own, a body that ties a
  knot, two modules incomparable and rendering as their signature, and each
  refusal.
- [`module/tests/coerced.rs`](module/tests/coerced.rs) — what a barrier holds, and `values`
  seeing it as the function it stands for.
- [`tests/copy.rs`](tests/copy.rs) — a two-node knot crossed under a copy is the
  same knot rebuilt, and so is a tagged ring beside a function capturing it
  (`a_copied_ring_is_the_same_graph_rebuilt`) and a module whose members are
  themselves knot members (`a_copied_module_is_the_same_members_rebuilt`).
- [`tests/equality.rs`](tests/equality.rs) — comparison and rendering: rings
  from two programs are equal, a list node holding a function is
  incomparable, and a ring renders with a label where it closes.
- [`tests/properties.rs`](tests/properties.rs) — over `scope`'s generated shape
  plans ([TEST.md](../../TEST.md#scope-property-laws)): a component of value
  binders that is cyclic or all callable ties iff the reads among its data
  members are acyclic, and refuses with `TypeCycle` naming data members
  otherwise; a tied data node holds one link per planned read and a tied
  function's closure follows its capture layout; and every tied knot copies
  whole with each link's kind kept.

Four tests join the koan [Miri slate](../../observe/miri_slate.md), the paths
only `knot` drives: `a_copied_knot_outlives_its_home` — a knot's node run
filled while closure runs and deep copies are written into the same region,
read through its edges after the region it was copied from is released —
`a_copied_ring_outlives_its_home`, the same for a tagged ring whose record
holds a string cell and an anonymous list node,
`a_copied_module_outlives_its_home`, the same for a module whose member run is
rebuilt member by member with each member's own knot behind it, and
`a_copied_barrier_outlives_its_home`, for a barrier's resident written beside
the node while the copy's node run is still being filled.

## Open work

- [Dispatch](../../roadmap/rewrite/dispatch.md) — calling a function, and
  builtins as function values with native bodies.
- [Module programs](../../roadmap/rewrite/modules.md) — a call through a
  barrier node, which coerces its arguments inwards and its return outwards.
- [The top level on the scheduler](../../roadmap/rewrite/top-level-on-the-scheduler.md)
  — a component tied as one unit by the body runner, whose eager parts it has
  evaluated and supplies to the tie by site.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — a function
  value born outside a binder's root, and union-variant construction in a
  cycle.
