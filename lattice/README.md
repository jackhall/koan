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
  interning registry of a frozen root and per-habitat branches, the identity recipe, the relations between types and the
  unifier ([src/types/README.md](src/types/README.md)). It names `symbols` and
  the bump tier, and nothing else.
- [`bump`](src/bump.rs) — the bump tier, below. It names neither of the
  others.

## The bump tier

Storage outside any cell graph, with no reach and no cell, released whole when
its owner drops. It holds the type lattice's root and the scratch a caller
passes. It is a *collections* arena: `BumpAllocator` is `&Bump`, both
bumpalo's verb receiver and the `Allocator` its growable `BumpVec` and its
hashbrown `BumpBackedMap`, `BumpHashTable` and `BumpBackedSet` are built over. A cell's region
has no verb that grows a buffer, so the tier does not pretend to be a region,
and it carries no door of its own: callers use bumpalo's `alloc`,
`alloc_slice_copy`, `alloc_slice_fill_iter` and `alloc_str`. The one
exception is `LaysRuns`, below, which a bump implements so that the digest
table can grow in one and a type tier can lay its nodes there.

**Drop-freeness is a compile-time fact.** Bumpalo runs no destructor, so
nothing with drop glue goes in. `bump_table`, `bump_hash_table` and `bump_set`
assert that for their entries as a `const` assert at each instantiation, so **an entry type
bringing drop glue with it fails the build where it is declared**, not at
runtime and not in review. A slice or a single value is the caller's to keep
`Copy`. The same discipline is why releasing a bump costs its chunks and
nothing more: no graph of owned values is walked on the way out.

## The digest table

[`DigestTable`](src/bump/digest_table.rs) is an insert-only hash table of
references to entries that each carry their own key (the `Keyed` trait) and
name a uniformly distributed low word of it: a 128-bit digest's own, or a
fold of a wider key, as a verdict entry folds its two digests and relation.
The word is already a hash, so the table runs no hash function, and the entry
holds its key, so the table stores no copy. A slot is 16 B: the key's word,
whose masked low bits are the slot's home, beside the reference. A probe
walks linearly to the first empty slot and reads an entry, comparing its whole
key, only when its word matches; the load stays at most one half, and the
smallest run is four slots, one cache line.

The table grows without an allocator. It lays a run of twice the length
through `LaysRuns` — a trait an insert takes as an argument, which lays a run
of values or a string — and re-places every slot by its stored word, so growth
reads no entry and strands the old run. A bump implements `LaysRuns` here;
koan implements it for a habitat half — a cell region's or scratch habitat's
`Writer` beside its header door — which lays fixed runs and has no allocator,
so a habitat can host the table. A type branch's table is one
([storage](src/types/identity.md#storage-a-root-and-its-branches)).

`cargo bench -p lattice --bench node_table` times the digest table against
the hashbrown table the root keeps.

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
- **One `unsafe` outside the test build.** The crate root carries
  `#![cfg_attr(not(test), deny(unsafe_code))]`, and the one site that allows
  it is the read behind a sealed component's member table
  ([`src/types/component.rs`](src/types/component.rs)), which the Miri slate
  pins ([TEST.md](../TEST.md)). The test build admits one more:
  [`src/tests.rs`](src/tests.rs) installs the counting global allocator
  from [`audit/counting_alloc.rs`](../audit/counting_alloc.rs) for the crate's
  own test binary, so the lattice's heap-contract tests bracket the
  allocations a call makes.

`cargo test -p lattice` runs the symbols tests, the lattice's property suite
and its heap-contract tests; [TEST.md](../TEST.md) is the testing reference.
The crate carries no roadmap tree: its open work is in koan's
([the type lattice's open work](src/types/README.md#open-work)).
