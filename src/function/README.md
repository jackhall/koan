# Functions

Functions as values: the layer that closes [`Value`](../values.rs)'s callable
parameter with a function node, ties a component of callable binders as one
[knot](../memory/README.md#the-knot), and copies a callable by re-tying its
knot. It sits above [`values`](../values/README.md),
[`scope`](../scope/README.md) and [`elaborate`](../elaborate/README.md), and
neither `values` nor `scope` names it.

## What a callable is

`values` does not define a callable. Its `Value<'graph, 'cell, X>` has one arm,
`Callable(X)`, over a parameter it asks two things of
([What a value is](../values/README.md#what-a-value-is)), and `scope` threads
the same parameter through closure bindings, the builtin table and the
activation. This module closes it:

- [`Callable`](../function.rs) is one member of a knot — the `(knot, index)`
  pair, sixteen bytes — so a value holding one stays a twenty-four-byte word.
- The knot's payload is a `Node`, whose one arm is a `Function`: the function's
  memoized type handle, the body shape it runs (in program storage), its
  closure bindings, and the weight of the whole knot it sits in.
- `KValue`, `KValueFamily`, `KValueCarrier` and `KActivation` spell the value,
  its family, its carrier and the activation at this parameter.

Every function is a knot node. A function that names no fellow is a one-node
knot, so there is one representation, one birth path and one copy. A node is
region-resident, `Copy` and so `Drop`-free, and a function's fields are private
to this module: a node exists only because the tie, or a copy of a tied knot,
laid it down.

## The tie

[`tie`](birth.rs) births one deferred-only component of a body's bindings
([Visibility](../scope/README.md#visibility)) as one knot in the writer's
region, and hands the knot back. **The caller binds**: member `i` of the
component is the knot's node `i`, and the caller binds each member's slot to
`Callable::of(knot, i)`. A component is therefore one unit of work that binds
several slots.

The tie runs in two passes.

1. **Stage, with no writer in reach.** For each member in component order: the
   body shape its binder births (`Shape::births`), refusing a member that
   births none; the function's type, elaborated from the form node its body
   sits in ([A callable's type](../elaborate/README.md#a-callables-type)); and
   its captures, read from the enclosing activation into scratch
   (`ClosureBindings::read_captures`). A capture of a fellow member is the edge
   the knot's plan mints for that member's index.
2. **Write.** Each member's closure run is laid down, the knot's weight is
   summed once, and the nodes are tied in member order.

Because nothing is written before every read has finished, **a refusal writes
nothing**. The refusal is an `Untieable`:

- `Data` — a member whose binder births no callable. A component with a data
  member is a circular value, not a knot of functions.
- `Pending` — a capture, or a type name in a member's signature, whose binder
  is still running, with that binder's cell handle. The caller waits on the
  binder and ties again.
- `Type` — a member's signature did not elaborate.

A binder births a callable when its right-hand side is a callable form at its
root, through transparent groups (`LET f = (FN …)`), or when its form is a
combined one (`LET f = FN EXPR …`, `LET f = OP …`). A callable form anywhere
else — inside a list, or called where it is written — is not a binder's and is
born by no tie.

## Closure bindings and edges

A function's captured environment is the scope layer's
[closure bindings](../scope/README.md#three-tiers): one run, in capture-slot
order, of value words and knot edges. A value word is the enclosing binding's
word, shallow, so capturing a string shares its bytes. An edge names a fellow
member by index.

No reader sees a bare edge. An activation of a callable holds the callable it
runs, and a read of a capture that is an edge resolves through it
(`Callable::sibling`) to the sibling callable. A block inside a callable's body
inherits its callable, so a nested function capturing an enclosing member reads
it as that sibling's value word, and captures a callable, not an edge.

## Weight and copy

A callable's weight is its knot's: the run header, one `Node` per member, and
each member's closure run with everything its captured values point at. It is
summed at the tie and memoized on every node, so a crossing prices a callable
by reading one field, under the ordinary
[verdict](../values/README.md#crossing).

**Every callable copies.** The value family's deep copy hands a callable to
this module's family together with the copy itself, and
[`copy_into`](copy.rs) re-ties the whole knot in the destination's region:
every node rebuilt in index order, each captured value deep-copied through the
copy the crossing priced, each edge carried verbatim — an edge names a node by
index, so it means the same node in the copy — and the type, body shape and
knot weight carried over. The copied callable is the member at the source's
own index. A copy never shares a node with its source, so a copied knot
outlives the region it was copied from.

## Equality and rendering

A callable has no structural equality: any comparison that reaches one is
`Incomparable` ([Equality](../values/README.md#equality-and-rendering)), which
the `==` builtin reports rather than answering `false`. A callable renders as
its type's name does, `:(FN :{x :Number} -> Number)`; its closure bindings are
program state and never print.

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
and brings each binding into being — a data binder whose right-hand side
lowers, a type binder whose right-hand side elaborates, a callable component
through the tie.

- [`tests/birth.rs`](tests/birth.rs) — a lone function, a captured value word,
  mutual and self recursion read back through their edges, a nested capture of
  an enclosing edge, and each refusal.
- [`tests/copy.rs`](tests/copy.rs) — a two-node knot crossed under a copy is the
  same knot rebuilt.
- [`tests/equality.rs`](tests/equality.rs) — comparison and rendering.
- [`tests/properties.rs`](tests/properties.rs) — over `scope`'s generated shape
  plans ([TEST.md](../../TEST.md#scope-property-laws)): every component of
  callable binders ties to one knot whose closures follow its members' capture
  layouts, a component with a data binder refuses, and every tied knot copies
  whole.

One test joins the koan [Miri slate](../../observe/miri_slate.md):
`a_copied_knot_outlives_its_home`, the path only `function` drives — a knot's
node run filled while closure runs and deep copies are written into the same
region, read through its edges after the region it was copied from is
released.

## Open work

- [Circular values](../../roadmap/rewrite/circular-values.md) — a knot with
  data members.
- [Modules](../../roadmap/rewrite/modules.md) — a module node beside the
  function node.
- [Dispatch](../../roadmap/rewrite/dispatch.md) — calling a function, and
  builtins as function values with native bodies.
- [Scheduler on cellgraph](../../roadmap/rewrite/scheduler-on-cellgraph.md) —
  a component submitted as one unit of work.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — a function
  value born outside a binder's root.
