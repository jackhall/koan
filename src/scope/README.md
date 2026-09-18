# Scopes

Koan's lexical environments: the layer that answers what a name means at the
point it is read, over the values in [`values`](../values/README.md) and the
types in [`type_lattice`](../type_lattice/README.md).

A scope resolves value names and type names. Keyword lookup — choosing a
callable for a keyworded expression — is dispatch, the layer above, and it
extends the resolution described here rather than replacing it.

## Three tiers

A scope is built in three tiers, each at the moment its contents become known.

- **The shape** — one per body, built once in program storage and shared by
  every scope instance of that body. It records the value names and type names
  the body declares, each with the lexical position its binder writes at, the
  class of every mention and the components its bindings form, and it
  resolves every name the body reads.
- **Closure bindings** — one run per callable, held in the callable value and
  built when the callable is born. Each slot is a `values::Link`, the one
  value-or-edge type a knot's data node holds too. Each name the body reads
  from an enclosing scope is copied in shallowly: the binding's value word, not
  a deep copy of what it points at. A name that is a member of the callable's
  own [component](#visibility) is held as an edge into the knot the component
  is born in, never as a copied value. A callable's birth reads every capture into
  scratch first and lays the run down only once none is pending, so a closure
  binding is never a placeholder and a refused birth writes nothing.
- **Per-call bindings** — one activation per call, laid down in the call's
  frame region: a pointer to the callable's closure bindings, the callable
  itself, and one slot for each parameter and each local the shape declares.
  Nothing is copied out of
  the closure bindings; a read of a capture goes through the pointer, one load
  more than a read of a local. Copying the captures in would cost a word per
  capture per call, multiplied by recursion depth, and a copied knot edge would
  need its knot carried beside it, where an edge left in its node resolves
  against the knot it already lives in.

Values are immutable, so a shallow copy of a binding means the same thing as a
reference to it: every copy names the same value, and what the copy retains is
the region that value lives in.

**Four kinds of shape.** The program's top level is one shape with no
captures. A function body (`FN`, `EXPR`, `OP`) is a *callable* shape: it
captures, and it is a deferring boundary (below). Its parameters are the names
its signature declares — each `<name> :<Type>` pair, a `:{…}` schema's fields,
every `FOR ALL` type parameter — or `left` and `right` for a binary `OP` and
`operands` for a unary one, all at position `0`. A `MODULE` or `GROUP` body is
a *module* shape: it captures, since its activation outlives the frame that
births it, but it is not a deferring boundary, because its statements run when
the statement holding it runs. A `MATCH` or `TRY` arm, and the code an `EVAL`
runs, is a *block* shape: an arm's one parameter is `it`, its statements count
from `1`, its activation is laid down in the same frame as the enclosing one,
and instead of captures it holds a pointer to the enclosing activation. A name
declared in the block shadows the enclosing one from the next statement on and
is gone once the block ends, because the block's activation is not the
enclosing one.

One builder serves all four kinds. It walks a body once, building every body
and arm nested in it as it meets them, and lays each finished shape down in
program storage. A nested shape is found from its enclosing one by the address
of the part that holds it, and a mention by the address of its own part — an
identity any holder of the part recomputes, and which program storage never
moves.

## Resolution

Every name a body reads resolves when its shape is built, to one of three
coordinates:

- a slot of the body's own per-call bindings,
- a slot of its closure bindings, read through the activation's pointer, or
- an index into the builtin table, read through the reader's own header.

A slot and a closure binding are prefixed by how many enclosing block
activations to step through first: none for a callable's own body, one per
block the reader sits inside. A builtin carries no such count, so a builtin
coordinate that steps out is not expressible. Reading a
name is then one indexed load, two for a capture, plus one per enclosing
block. No runtime walk visits an enclosing *scope* by name, and no binding
holds a reference into another scope: a coordinate is computed from the name
at the reading site, and what a slot holds is a value or an edge into the
holder's own knot.

A name a callable body reads from outside is added to its capture layout once,
however often the body reads it, with where the callable's birth reads it from:
a coordinate of the enclosing activation, or — when the name is a fellow member
of the component the callable's own binding belongs to — that member's place in
the component, which the birth turns into a knot edge the caller mints.

**No reader sees a bare edge.** The scope layer is generic over the knot member
a value holds — the parameter [`values`](../values/README.md#what-a-value-is)
takes — and reads one only to resolve an edge. An activation of a callable
holds the member it runs, and a block inside it inherits that member, so a
read of a capture that is an edge resolves through it to the sibling the edge
names — a function or a data node — a bound value like any other. A capture read at birth through
such a coordinate is therefore the sibling's value word.

A search by symbol happens only where the shape is built and inside `EVAL`.
How the shape's runs are searched — linear below some length, binary above —
is an implementation detail measured, not a commitment of the design.

## Visibility

A binding is visible to a reader when its declared position is strictly less
than the reader's, compared within the scope that declares it. A parameter
writes at position `0` and a body's statement `i` at `i + 1`. Where a reader
reads depends on what it does with the name.

**Eager and deferred mentions.** Every mention of a name in a binding's
right-hand side is classified by its path from the right-hand side's root. A
mention is **deferred** when that path is non-empty and every context on it,
down to the outermost callable-body boundary it crosses, is a constructor slot
(a list element, a dict value, a record field) or that boundary itself: the
value is only stored in a container or captured by a callable, never
inspected. A type declaration's schema — a `UNION`'s variants, a `SIG` or
`NEWTYPE`'s fields — is a constructor slot too, so a type naming itself or a
later sibling in its schema is a deferred mention. So is a nominal
construction's payload: in a two-part node whose head is a type name,
`(Ring {next = a})`, the head is an eager mention and the payload a constructor
slot, so a tagged value naming itself reaches the tie rather than being an
eager cycle. A type-constructor application written in value position has the
same shape and is classified the same way; the tie, not the shape, refuses it
when it closes a cycle. A union variant's construction (`Tree.Node x`) is a
attribute form and stays eager. A callable body is opaque — nothing inside it changes the class,
since none of it runs until the callable is called. Any other mention is
**eager**: the value is needed at the point it is read. A call, a keyword
form's slot, an operator's operand, a dict key, a type expression outside a
schema, a `MODULE` body and a `MATCH` or `TRY` arm are all eager contexts, and
so are a callable's parameter and return types, which are mentions of the
enclosing shape read where the callable is born. A parenthesized group of one
part is transparent: it is the part. So in

```
LET f = FN <reads g>
LET x = [f, compute(5)]
LET y = g([f])
LET z = (FN <reads f>)()
LET a = b
```

`f`'s mention of `g` is deferred, however `g` is used inside the body, as is
`x`'s mention of `f`; `x`'s mention of `compute` is eager. `y`'s mention of
`f` is eager, because the list sits under a call. `z`'s mention of `f` is
eager although it sits in a body, because that body is called. `a`'s mention
of `b` is eager: the root itself is the mention.

An eager mention reads at its statement's position, so it sees every binding
declared before the statement and never a later one, and a binder never sees
its own right-hand side. A deferred mention reads at the body's end, so it
sees every binding the body declares, in any source order. Both positions are
fixed where the statement is written, not where a callable it births is
called. A function body therefore sees its own name, every sibling declared
after it, and everything declared before it; a forward *use* of a name is an
unbound-name error.

**Components.** With every mention resolved, the shape's bindings form a
reference graph, and the shape computes its strongly connected components, as
the [type lattice](../type_lattice/README.md#recursive-groups-identity-is-the-scc-not-the-declaration)
does for a group of recursive types. A component whose internal mentions are
all deferred is a group of values that only store and capture one another. Its
members can be born together as one [knot](../memory/README.md#the-knot) once
each member's eager mentions, which all leave the component, have evaluated:
a member's reference to a sibling or to itself is an edge, never a wait on a
pending slot. Two mutually recursive functions form such a component, and so
do a ring of containers and a container holding a callable that captures it.
A lone self-recursive function is a one-member component.

A component containing an eager mention of a fellow member has no solution:
the mentioning binding cannot finish until the member's value exists, and the
member's value cannot exist until the component is tied, which waits on the
mentioning binding. The shape rejects such a program where it is built, never
a hang. `LET a = b; LET b = a` is the smallest case, an eager mention at each
root; `LET f = FN <reads x>; LET x = f()` is the same error through a call,
and so is a function outside a module that is mutually recursive with one
inside it, since a module body is an eager context — the two belong in one
module.

The shape hands the layer above each body's components — each with whether it
is `deferred_only` and whether it is `cyclic`, holding more than one member or
a member that reads itself — and the class of every mention, and three facts a
tie reads: the callable body each binder births — `Shape::births`, set when the
binder's right-hand side is a callable form at its root or its form is a
combined one — `Shape::form`, the form node a callable body sits in, where its
signature is read, and `Shape::rhs`, each `LET` binder's right-hand side part,
where a data member is read. A caller ties a component of value binders when
it is cyclic or every member births a callable; a non-cyclic data binder is an
ordinary value, and a component of type binders is the elaborator's. A
component never mixes the two channels: a schema names types only, so no
mention leaves a type binder for a value binder. Tying is
[`function`](../function/README.md#the-tie)'s, which writes a deferred mention
below a nested constructor into the knot as an anonymous node.

## Two channels

A value name and a type name are different key types, so the value channel and
the type channel cannot collide by construction. A name whose text classifies
as neither is rejected where the text is classified, before any scope sees it.
The partition lives in the shape: each channel is its own run of declared
names, sorted by symbol, and the two share one index space — value names take
the first slots and type names the slots after. The builtin table lays its two
channels out the same way. An activation holds one run of slots over `Value`, since a type is a
`Value` arm, and the shape's key types keep the two channels' indices apart.
Keyword buckets are dispatch's to resolve.

**Builtins are immutable and unshadowable.** A user binding whose name collides
with a builtin's, in either channel, is a rebind error at any depth, never a
shadow. Because no scope can hide a builtin, a builtin name resolves in the
shape to an index into the builtin table, and a read goes through a base
pointer to that table carried in the activation's header. An activation copies
no part of the builtin table.

## Placeholders and writes

A slot is written once. Its binder replaces the placeholder in place, and
nothing rewrites it after that, so a binding is as immutable as the value it
holds.

A pending slot names its binder: the
[`CellHandle`](../../cellgraph/src/handle.rs) of the cell that will bind it. A
read of a visible slot returns the bound value, or the pending handle; the
embedder turns the handle into a dependency edge from the reading cell to the
binder, and the scope parks nothing itself. The waiter chain is the
scheduler's dependency graph, not the slot's.

A slot visible to a running reader is never unwritten. A deferred mention
reads at the body's end and sees the siblings declared after it, so the
scheduler submits every statement of a body, in position order and claiming
each binder's slot as it submits it, before any of those statements runs.
Empty is a state a slot has only before its binder is submitted, and a read
that finds one is a scheduler bug, not a pending read.

A function activation and a module activation are one shape. A module body's
binders are its exports in flight, and a `USING` over a binder that is still
pending is the same pending read.

## Names that arrive at run time

Two forms introduce names no shape can see.

- **`EVAL`** builds a block shape for the code it evaluates when it runs,
  over the activation it appears in: every free name resolves by name, a
  search of each enclosing shape's declared names from the innermost outward
  with the same visibility rule, reading the enclosing shape at `EVAL`'s own
  position as an eager mention there would. A by-name resolution picks the same binding the
  shape's coordinate would. A binder in the evaluated code binds in that block
  shape and is gone when it ends; nothing an `EVAL` runs declares into the
  scope around it. Resolving outward needs the enclosing scopes to still
  exist, so a shape containing `EVAL`, and every shape lexically enclosing
  it, retains its defining scope. Every other shape resolves through
  coordinates alone and keeps no link to its parent.
- **`USING … SCOPE`** takes the names it surfaces from the module's signature,
  so a shape resolves them like any other name. The module's signature must be
  known statically where `USING` appears; a module whose signature is not
  requires an ascription there.

## Errors

A shape that cannot be built is refused where it is built, with the first
error in walk order:

- a **rebind** — a name declared twice in one shape, parameters included;
- a binding that **shadows a builtin**, in either channel;
- an **unbound** name — no binding of it visible where the mention reads;
- an **eager cycle** — a component with an eager mention of a fellow member;
- an **unsupported** form — `USING … SCOPE`, `CLOSE` and `CLOSE OVER`, whose
  resolution has no rewrite home yet, and the reserved forms that exist only
  to diagnose a miss;
- a **malformed** form — a body or a branch list that is not the shape its form
  declares.

Each renders with the names and positions a user needs, spelled through the
label interner.

## Memory

An activation is `Drop`-free and laid down in its frame's region, so a frame's
death releases it with the region. It is a header of four pointers — its shape,
its closure bindings, the builtin table and, for a block, the enclosing
activation — beside the callable it runs, if any, and one slot array over the
shape's slot count. It holds no
pointer into itself: its shape lives in program storage, its builtin table
outlives every frame, its closure bindings live in the callable value, which
the caller keeps alive across the call, an enclosing activation lives in the
same frame, and its slots hold values. An activation is therefore copied by
copying its bytes. Each kind of body has its own constructor — a program's
with neither closure bindings nor an enclosing activation, a callable's or
module's with the callable and its closure bindings, a block's beside an
enclosing activation whose builtin table and callable it shares — so no other
combination can be built.

A shape and everything it holds — declared-name runs, mentions, captures,
components, nested shapes — rest in program storage and are `Copy`. An
`EVAL`'s block shape is built into program storage each time the `EVAL` runs.

## The import rule

`scope` names `crate::values`, `crate::type_lattice`, `crate::memory` and
`crate::parse`, and no scheduler type. A pending slot's cell handle is
`cellgraph`'s name for a unit of work, spelled through `memory`'s substrate
re-exports like every other substrate name; the scheduler reaches scopes
through its embedder, and scopes never reach the scheduler. The compiler
cannot hold a module to that, so [`tests::boundary`](tests/boundary.rs) reads
the module's source and fails on any other `crate::` path, on an owning heap
type outside the one error that lists names, and on a retired lifetime name.

## Open work

- [Scheduler on cellgraph](../../roadmap/rewrite/scheduler-on-cellgraph.md) —
  which habitat each tier of an activation is laid down in, and how a reader
  parks on a binder that has not run.
- [Modules](../../roadmap/rewrite/modules.md) — `USING … SCOPE` resolved
  through a module's signature.
- [Dispatch](../../roadmap/rewrite/dispatch.md) — keyword lookup over scopes.
- [Unplanned work](../../roadmap/rewrite/README.md#unplanned-work) — `CLOSE
  OVER`, and an `EVAL` retaining its defining scope across frames.
