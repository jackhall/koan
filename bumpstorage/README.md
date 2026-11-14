# bumpstorage

The bump tier: storage outside any cell graph, with the tables laid in it and
the component walk staged in it.

The [type lattice](../lattice/README.md), [`bindgraph`](../bindgraph/README.md)
and [koan](../README.md) rest on it, and it depends on none of them, so
neither the lattice nor `bindgraph` reaches the other's code to reach its
storage. Koan re-exports it under `crate::memory`
([the storage tiers](../src/memory/README.md)).

## The bump tier

Storage outside any cell graph, with no reach and no cell, released whole when
its owner drops. It holds the type lattice's frozen load and agent tier, the
binding graph's instances and views, and the scratch a caller passes. It is a
*collections* arena: `BumpAllocator` is `&Bump`, both bumpalo's verb receiver
and the `Allocator` its growable `BumpVec` and its hashbrown `BumpBackedMap`,
`BumpHashTable` and `BumpBackedSet` are built over. A cell's region has no verb
that grows a buffer, so the tier does not pretend to be a region, and it
carries no door of its own: callers use bumpalo's `alloc`, `alloc_slice_copy`,
`alloc_slice_fill_iter` and `alloc_str`. The one exception is `LaysRuns`,
below, which a bump implements so that the digest table can grow in one and a
type tier or a binding graph can lay its entries there.

**Drop-freeness is a compile-time fact.** Bumpalo runs no destructor, so
nothing with drop glue goes in. `bump_table`, `bump_hash_table` and `bump_set`
assert that for their entries, and a bump's `LaysRuns::lay` for its run, as a
`const` assert at each instantiation, so **an entry type bringing drop glue
with it fails the build where it is declared**, not at runtime and not in
review. A single value laid by bumpalo's own verbs is the caller's to keep
`Copy`. The same discipline is why releasing a bump costs its chunks and
nothing more: no graph of owned values is walked on the way out.

## The digest table

[`DigestTable`](src/digest_table.rs) is an insert-only hash table of
references to entries, each homed by a uniformly distributed 64-bit word: a
128-bit digest's low word, or a fold of a wider key, as a verdict entry folds
its two digests and relation. The word is already a hash, so the table runs no
hash function. A slot is 16 B: the word, whose masked low bits are the slot's
home, beside the reference. A probe walks linearly to the first empty slot and
tests an entry only when its word matches; the load stays at most one half,
and the smallest run is four slots, one cache line.

An entry is found in one of two ways, over the one probe:

- **By its key.** A `Keyed` entry carries its own `Copy` key and names the
  key's word, so the table stores no copy of the key: `get` and
  `insert_unique`.
- **By a word and a test.** An entry whose key a caller builds for one lookup,
  such as a slice no `Copy` key could carry, is found by its word and a test
  the caller supplies, and inserted under a word the caller computes: `find`
  and `insert_at`. The binding graph's answer and view entries are found this
  way, by a handle's words or a view's digests.

The table grows without an allocator. It lays a run of twice the length
through `LaysRuns` — a trait an insert takes as an argument, which lays a run
of values or a string — and re-places every slot by its stored word, so growth
reads no entry and strands the old run. A bump implements `LaysRuns` here;
koan implements it for a habitat half — a cell region's or scratch habitat's
`Writer` beside its header door — which lays fixed runs and has no allocator,
so a habitat can host the table. A type branch's table is one
([storage](../lattice/src/types/identity.md#storage-a-root-and-its-branches)).

[`FrozenTable`](src/frozen_table.rs) is a digest table's read-only twin, laid
once in a bump by `placed` (from `Keyed` entries) or `placed_at` (from entries
beside their words, as a digest table's `words` yields them): the same slots in
the same places, probed by the same function, but a plain slice that is never
written, so it holds no cell and is `Sync` wherever its entries are. The type
lattice's frozen load indexes its nodes and its verdicts with two, and a
binding graph's frozen part its answers and its views. A generated image
writes a frozen table's slots as a `static` run; the `generator` feature, off
by default, opens those slots to the lattice's image generator across the
crate boundary.

## Strongly connected components

[`strongly_connected_components`](src/components.rs) is Tarjan's walk over an
index graph: `edges[i]` lists the nodes `i` references, and the components
come back as runs of node indices, every buffer staged in a bump the caller
passes. The emission order is reverse topological on the condensation — a
component comes out only after every component it references — which is the
order a caller that finishes each component against the ones below it relies
on. The walk keeps its own stack of frames in the bump rather than recursing,
since a program's chain of bindings is as long as its author writes it.

It lives in the bump tier because it names nothing of what a node stands for
and has two callers that must not depend on each other: the
[type lattice](../lattice/src/types/identity.md#recursive-groups-identity-is-the-scc-not-the-declaration)
condenses a recursive group's members to digest them, and koan's
[scope shape](../src/scope/README.md#visibility) condenses a body's bindings
to find the components a knot can tie and the eager cycles it refuses.

## The import rule

- **The crate depends on no workspace crate.** Its manifest names none, so
  the edge holds by the build.
- **It is the workspace's only import of `bumpalo`, `hashbrown` and
  `allocator_api2`.** Inside the crate, [`lib.rs`](src/lib.rs) is the only
  file naming them.
- **No `unsafe`.** The crate root carries `#![forbid(unsafe_code)]`; the
  `unsafe` under it is bumpalo's, which koan's Miri slate drives through the
  lattice's tables ([TEST.md](../TEST.md)).

`cargo test -p bumpstorage` runs the digest tables' tests and the component
walk's law. The crate carries no roadmap tree.
