# Functions

Functions as values, and the knots they are born in: the layer that closes
[`Value`](../values.rs)'s knot-member parameter with a node that is a function
or a data node, ties a component of value binders as one
[knot](../memory/README.md#the-knot), and copies a member by re-tying its knot.
It sits above [`values`](../values/README.md), [`scope`](../scope/README.md)
and [`elaborate`](../elaborate/README.md), and neither `values` nor `scope`
names it.

## What a knot member is

`values` does not define a knot member. Its `Value<'graph, 'cell, X>` has one
arm, `Knotted(X)`, over a parameter it asks two things of
([What a value is](../values/README.md#what-a-value-is)), and `scope` threads
the same parameter through closure bindings, the builtin table and the
activation. This module closes it:

- [`Knotted`](../function.rs) is one member of a knot — the `(knot, index)`
  pair, sixteen bytes — so a value holding one stays a twenty-four-byte word.
  Its equality is node identity.
- The knot's payload is a `Node`. A `Function` node holds the function's
  memoized type handle, the body shape it runs (in program storage), its
  closure bindings, and the weight of the whole knot it sits in. A `Data` node
  holds a [`Circular`](../values/circular.rs) — a list, dict, record or tagged
  resident whose cells are links — and the same knot weight.
  `values::Knotted::resolve` tells the two apart, and `Knotted::function`
  answers only for the first.
- `KValue`, `KValueFamily`, `KValueCarrier` and `KActivation` spell the value,
  its family, its carrier and the activation at this parameter.

Every function is a knot node. A function that names no fellow is a one-node
knot, so there is one representation, one birth path and one copy. A node is
region-resident, `Copy` and so `Drop`-free, and a node's fields are private
to this module: a node exists only because the tie, or a copy of a tied knot,
laid it down.

## The tie

[`tie`](birth.rs) births one deferred-only component of a body's bindings
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
nominal construction `(Head payload)`. Anything else is `Opaque` and refuses
the tie before a member is read. A callable form anywhere else — inside a list,
or called where it is written — is not a binder's and is born by no tie.

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

- `Opaque` — a member that is neither a function member nor a data member.
- `Pending` — a read whose binder is still running — a capture, a type name in
  a signature, a data member's mention, a construction's head — with that
  binder's cell handle. The caller waits on the binder and ties again.
- `Eager` — a part of a data member at a site the evaluator has no value for.
  The caller evaluates it and ties again.
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
function's closure run and each data node's resident, with everything their
value words point at. It is summed at the tie and memoized on every node, so a
crossing prices a member by reading one field, under the ordinary
[verdict](../values/README.md#crossing).

**Every knot member copies.** The value family's deep copy hands a member to
this module's family together with the copy itself, and
[`copy_into`](copy.rs) re-ties the whole knot in the destination's region:
every node rebuilt in index order — a function's closure run through
`ClosureBindings::copied`, a data node through `Circular::copied` — each held
value deep-copied through the copy the crossing priced, each edge carried
verbatim, since an edge names a node by index and so means the same node in
the copy, anonymous nodes included, and the types, body shapes and knot weight
carried over. The copied member is the one at the source's own index. A copy
never shares a node with its source, so a copied knot outlives the region it
was copied from.

## Equality and rendering

A function has no structural equality: any comparison that reaches one is
`Incomparable` ([Equality](../values/README.md#equality-and-rendering)), which
the `==` builtin reports rather than answering `false`. A function renders as
its type's name does, `:(FN :{x :Number} -> Number)`; its closure bindings are
program state and never print. A data node compares as a bisimulation and
renders with `@n` labels where a cycle closes, both in `values`.

## The import rule

**Outside doc comments and `#[cfg(test)]`, `function` names `crate::elaborate`,
`crate::memory`, `crate::parse`, `crate::scope`, `crate::type_lattice` and
`crate::values`, and nothing else in the crate.**
[`tests::boundary`](tests/boundary.rs) reads this module's own source and fails
on any other `crate::` path, on an owning heap type outside the tests, and on a
retired lifetime name. `values`' and `scope`'s own boundary tests hold them
below this module.

## Testing

The suites run programs through a fixture that activates a program in a cell
and brings each binding into being — a lone data binder whose right-hand side
lowers, a type binder whose right-hand side elaborates, a cyclic component of
value binders or a component of callable binders through the tie with an
evaluator that supplies nothing. `elaborate` handles no `NEWTYPE`, so a test
that constructs one seals it as a singleton recursive group and hands it in as
a builtin type.

- [`tests/birth.rs`](tests/birth.rs) — a lone function, a captured value word,
  mutual and self recursion read back through their edges, a nested capture of
  an enclosing edge; a tagged self-reference and a two-member tagged ring, a
  container sharing a knot with the function that captures it, an anonymous
  node on a sibling path, a nested construction built through the checked
  door, an eager part supplied by site; and each refusal.
- [`tests/copy.rs`](tests/copy.rs) — a two-node knot crossed under a copy is the
  same knot rebuilt, and so is a tagged ring beside a function capturing it
  (`a_copied_ring_is_the_same_graph_rebuilt`).
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

Two tests join the koan [Miri slate](../../observe/miri_slate.md), the paths
only `function` drives: `a_copied_knot_outlives_its_home` — a knot's node run
filled while closure runs and deep copies are written into the same region,
read through its edges after the region it was copied from is released — and
`a_copied_ring_outlives_its_home`, the same for a tagged ring whose record
holds a string cell and an anonymous list node.

## Open work

- [Module values](../../roadmap/rewrite/module-values.md) — a module node beside
  the function node, and the tie that births one.
- [Dispatch](../../roadmap/rewrite/dispatch.md) — calling a function, and
  builtins as function values with native bodies.
- [The top level on the scheduler](../../roadmap/rewrite/top-level-on-the-scheduler.md)
  — a component submitted as one unit of work, whose eager parts the step
  evaluates and supplies to the tie by site.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — a function
  value born outside a binder's root, and union-variant construction in a
  cycle.
