# lattice

The symbol vocabulary and the type lattice over it. Both rest on the bump
tier, [`bumpstorage`](../bumpstorage/README.md): storage outside any cell
graph, its digest tables and the component walk.

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
  interning registry of a frozen load, an agent tier and per-habitat branches, the identity recipe, the relations between types and the
  unifier ([src/types/README.md](src/types/README.md)). It names `symbols` and
  the bump tier, and nothing else.

## The import rule

- **The crate depends on no koan code.** Its manifest names none, so the edge
  holds by the build: a path into koan does not compile.
- **No file here names `bumpalo`, `hashbrown` or `allocator_api2`.** They are
  [`bumpstorage`](../bumpstorage/README.md#the-import-rule)'s, and the crate
  reaches them only through it. `blake3` is the content digest under symbol and
  type identity here, and koan depends on it in its own right for a value's
  [content digest](../src/values/README.md#content-digests).
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
The `counters` feature, off by default, exposes the counters koan's tests read
across the crate boundary, where `cfg(test)` does not reach — a branch's merge
reach steps and its verdict count, and an adoption's asks, probes, hits and
nodes laid. Koan's dev-dependency turns it on, so a test build lays a branch a
word larger than the shipped interpreter's. The `bootstrap-image` feature
compiles the seed's hand-written stub in place of the generated seed, so the
seed's generator runs when a layout change stops the generated file compiling
([TEST.md](../TEST.md#generated-images)). The `generator` feature, off by
default, compiles the image generator — the emitter, the digest checks and the
golden compare — for koan's builtin-image test, which calls it across the crate
boundary; koan's dev-dependency turns it on beside `counters`, so the shipped
interpreter carries none of it. It turns on `bumpstorage`'s own `generator`
feature, which opens the frozen table's slots the emitter writes.
`cargo bench -p lattice --bench node_table` times the digest table against
the hashbrown table the root builder and the agent tier keep.
The crate carries no roadmap tree: its open work is in koan's
([the type lattice's open work](src/types/README.md#open-work)).
