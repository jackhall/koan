# The AST in `cellgraph` storage

The parsed AST is written through a `Writer`, into the one store program
storage holds.

**Problem.** [Program storage](../../src/memory/program.rs) holds two stores: a
`bumpalo` bump the parser writes the AST into, and the `cellgraph`-owned
`Storage` the [top level](top-level-on-the-scheduler.md) lays the builtin table
and the top-level slot array down in. `ProgramBrand::allocator` hands the bare
bump out, so [`parse`](../../src/parse/README.md) and
[`scope`](../../src/scope/README.md) allocate AST nodes, rewritten statements
and group claims with bumpalo's own verbs — `alloc`, `alloc_str`,
`alloc_slice_copy`, `alloc_slice_fill_iter` — which check nothing about what
they are handed. A node type that grew drop glue would leak silently, where
`Writer`'s verbs refuse one at compile time. The
[slot layout](../../src/parse/builtin_shapes/layout.rs) builds its entries in a
bump-backed vector over the same bump.

**Acceptance criteria.**

- `ProgramStorage` holds one store, the `cellgraph`-owned `Storage`, and
  `ProgramBrand` exposes a `Writer<'graph>` and no bump allocator.
- Every AST constructor in `parse` and every program-storage allocation in
  `scope` takes a `Writer`, so an AST node type carrying drop glue is a compile
  error at the constructor that writes it.
- The slot layout builds its entries through a `Writer` run.
- `parse` and `scope` import no bump-tier name from `memory` for program
  storage; the [bump tier](../../src/memory/bump.rs) serves the type lattice's
  registry and step scratch only.
- A boundary test pins that `ProgramBrand` has no method returning a
  `BumpAllocator`.

**Directions.**

- *Whether `Writer` gains a single-value verb — open.* The parser's commonest
  allocation is one node. `memory`'s `resident` already derives it from
  `Writer::fill` at length one; the alternative is a `cellgraph` verb.
  Recommended: `resident`, since `cellgraph` keeps its verbs to the shapes no
  embedder can derive.
- *The old runtime's callers — decided.* `src/machine` and `src/builtins` call
  the AST constructors with a bare bump. They are behind `pending_rewrite`, do
  not build, and are left stale.

## Dependencies

**Requires:**

- [The top level on the scheduler](top-level-on-the-scheduler.md) — it adds the `cellgraph` `Storage` this moves the AST into.

**Unblocks:** none — a leaf.
