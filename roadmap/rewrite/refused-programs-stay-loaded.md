# A refused program stays loaded

**Problem.** [`CellSubstrate::load`](../../src/program/substrate.rs) stands a
program up inside `self_cell`'s builder, and a builder that returns an error
drops the owner with it: the program's storage, its parse, its symbol interner
and its type registry. A shape error names symbols and slot types through the
interner and registry, so the load renders it into text before they go
(`LoadError::Shape`'s `rendered`), and `program`'s
[boundary test](../../src/program/tests/boundary.rs) allows that one `String`.
Nothing else of a refused program survives its load. A REPL or a debugger
cannot render the error again or differently, read the refused parse, or name
anything the program declared.

**Acceptance criteria.**

- A load refused at its shape hands back the refused program, still owning its
  storage, parse, interner and registry, held by the substrate that holds a
  loaded program.
- A refused program's shape error renders on demand, names and slot types
  included, through the program that holds it.
- `LoadError::Shape` holds no rendered text, and `program`'s boundary test
  allows no owning heap type.

**Directions.**

- *One owner for a loaded and a refused program — decided.* The refused program
  stays in the substrate's own `self_cell` rather than moving to a second one.
- *What `cellgraph` provides for it — open.* A refused program has no program
  record and runs nothing, so it needs its storage but not a drain over it.
  Whether the substrate still stands a graph up for it, or `cellgraph` holds
  program storage apart from a graph, is open.

## Dependencies

**Requires:**

- [`LoadError`](../../src/program/record.rs) — shipped: the refused load's
  rendered shape error this item retires.

**Unblocks:** none — a leaf.
