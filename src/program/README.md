# The program

A loaded program as one owning value. [`CellSubstrate`](substrate.rs) holds
program storage, the symbol interner, the type registry, the parsed program and
the [cell graph](../scheduler/README.md) that runs it, and it can be returned
from a function, moved, kept in a struct and dropped like any other value. An
embedder that keeps a program across calls — a language server, a REPL — keeps
one of these, and repeats no setup order of its own.

## Owner and dependent

The graph's cells, the registry and the AST all borrow program storage at
`'graph`, so a value holding the storage beside them is self-referential.
[`self_cell`](https://docs.rs/self_cell) carries the `unsafe` that takes;
koan's production code writes none. Its dependent is one lifetime and no type
parameters, so the substrate is a concrete type over koan's own families rather
than a generic `cellgraph` type.

- **The owner** is the permanent tier and nothing else: program storage and the
  interner. It borrows nothing, and `self_cell` boxes it and only ever lends it
  shared.
- **The dependent**, `Running<'graph>`, is everything that names `'graph`:
  - the **graph**, by value. Reclaiming one of its recycling regions needs
    exclusive access, which a shared borrow of the owner cannot give.
  - the **type registry**, laid down in program storage rather than held by
    value, so a record laid down there can borrow it at `'graph`. A registry
    held in the dependent could not be borrowed by a sibling, and would reach a
    step only through a per-call channel on the step context. Resting in a bump
    is sound because the registry owns nothing on the global heap — its
    [verdict table](../type_lattice/README.md#storage-one-region) is a fixed
    cache in the registry's own bump — so its destructor never needs to run.
  - the **parsed program's** top-level statements, copied into program storage.
  - the program brand and a borrow of the interner, so a call reaches every
    piece at `'graph` without touching the owner.

Anything else that carries `'graph` and has to outlive a single call belongs in
the dependent too.

## Nothing outside names `'graph`

`'graph` is invariant, so `Running` is declared `#[not_covariant]` and the
substrate exposes no signature that names it. The running state is reached
only through `CellSubstrate::with`, whose closure is quantified over a fresh
lifetime: nothing borrowed at `'graph` escapes a call, and two substrates'
states can never be mixed. Each call's `'graph` names the same storage, so what
one call leaves in the graph or interns in the registry the next call finds.

**Setup runs in the builder.** `load` parses the source, lays the statements
and the registry in program storage and stands the graph up inside
`self_cell`'s builder closure, which sees the same `'graph` every later call
does. A parse error stops the builder, and `load` returns it.

## The scheduler is a view

A [`Scheduler`](../scheduler/README.md#the-drain) borrows the graph and owns
none, so `Running::scheduler` makes a fresh drain over the substrate's graph for
the length of one call. A call whose drain stalls is accepted as it stands: the
view drops what is on its stack and the graph keeps the cells already born,
under the root they were born under. A stalled substrate is only ever dropped.

## Imports and tests

Outside `#[cfg(test)]` this module names `crate::memory`, `crate::parse`,
`crate::scheduler`, `crate::symbols` and `crate::type_lattice`; its tests may
also name `crate::knot` for the values their steps carry. `tests/boundary.rs`
reads the source to hold the rule there. `tests/substrate.rs` loads two
programs through a helper, moves them, and runs each across two separate calls;
it is on the [Miri slate](../../observe/miri_slate.md).

## Open work

- [The top level on the scheduler](../../roadmap/rewrite/top-level-on-the-scheduler.md)
  — koan's own steps, the builtin table and the `Program` record, built in the
  substrate's builder and kept beside the graph.
