# lattice

The symbol vocabulary, the type lattice over it, and the bump tier both rest on.

Koan ([README.md](../README.md)) is the crate's user. The crate depends on no
koan code, so nothing koan's evaluator knows — a value, a cell, an AST node, a
scope — is nameable here. That is what keeps the type lattice a closed algebra:
a relation that needed a value would be a relation the lattice could not state
as a law over generated types, and the compiler, not a review, refuses one.

## The modules

- [`symbols`](src/symbols.rs) — symbol identity: the classified newtypes every
  syntactic name travels as, the interner that turns one back into text, and
  the `static_name!` declaration over them
  ([src/symbols/README.md](src/symbols/README.md)). It names nothing else in
  the crate.
- [`types`](src/types.rs) — the type lattice: the node vocabulary, the
  interning registry, the identity recipe, the relations between types and the
  unifier ([src/types/README.md](src/types/README.md)). It names `symbols` and
  the bump tier, and nothing else.
- [`bump`](src/bump.rs) — the bump tier, below. It names neither of the
  others.

## The bump tier

Storage outside any cell graph, with no reach and no cell, released whole when
its owner drops. It holds the type lattice's registry and the scratch a caller
passes. It is a *collections* arena: `BumpAllocator` is `&Bump`, both
bumpalo's verb receiver and the `Allocator` its growable `BumpVec` and its
hashbrown `BumpBackedMap` and `BumpBackedSet` are built over. A cell's region
has no verb that grows a buffer, so the tier does not pretend to be a region,
and it carries no door of its own: callers use bumpalo's `alloc`,
`alloc_slice_copy`, `alloc_slice_fill_iter` and `alloc_str`.

**Drop-freeness is a compile-time fact.** Bumpalo runs no destructor, so
nothing with drop glue goes in. `bump_table` and `bump_set` assert that for
their entries as a `const` assert at each instantiation, so **an entry type
bringing drop glue with it fails the build where it is declared**, not at
runtime and not in review. A slice or a single value is the caller's to keep
`Copy`. The same discipline is why releasing a bump costs its chunks and
nothing more: no graph of owned values is walked on the way out.

## Strongly connected components

[`strongly_connected_components`](src/bump/components.rs) is Tarjan's walk
over an index graph: `edges[i]` lists the nodes `i` references, and the
components come back as runs of node indices, every buffer staged in a bump
the caller passes. The emission order is reverse topological on the
condensation — a component comes out only after every component it references
— which is the order a caller that finishes each component against the ones
below it relies on. The walk keeps its own stack of frames in the bump rather
than recursing, since a program's chain of bindings is as long as its author
writes it.

It lives in the bump tier because it names nothing of what a node stands for
and has two callers that must not depend on each other: the
[type lattice](src/types/identity.md#recursive-groups-identity-is-the-scc-not-the-declaration)
condenses a recursive group's members to digest them, and koan's
[scope shape](../src/scope/README.md#visibility) condenses a body's bindings
to find the components a knot can tie and the eager cycles it refuses.

## The import rule

- **The crate depends on no koan code.** Its manifest names none, so the edge
  holds by the build: a path into koan does not compile.
- **No koan file names `bumpalo`, `hashbrown` or `allocator_api2`.** They are
  this crate's dependencies, not koan's. Inside the crate,
  [`bump.rs`](src/bump.rs) is the only file naming them. `blake3` is the content
  digest under symbol and type identity here, and koan depends on it in its own
  right for a value's [content digest](../src/values/README.md#content-digests).
- **No `unsafe` outside the test build.** The crate root carries
  `#![cfg_attr(not(test), forbid(unsafe_code))]`. The test build admits one
  site: [`src/tests.rs`](src/tests.rs) installs the counting global allocator
  from [`audit/counting_alloc.rs`](../audit/counting_alloc.rs) for the crate's
  own test binary, so the lattice's heap-contract tests bracket the
  allocations a call makes.

`cargo test -p lattice` runs the symbols tests, the lattice's property suite
and its heap-contract tests; [TEST.md](../TEST.md) is the testing reference.
The crate carries no roadmap tree: its open work is in koan's
([the type lattice's open work](src/types/README.md#open-work)).
