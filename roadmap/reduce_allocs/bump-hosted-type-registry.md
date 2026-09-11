# Bump-hosted type registry

**Problem.** The run's type registry owns its content on the global heap. The node table is a
persistent HAMT over `Rc` ([registry.rs](../../src/type_lattice/registry.rs)), and every compound
[`TypeNode`](../../src/type_lattice/node.rs) owns heap payload: a union's member `Vec`, a shape's
quantifier `Vec` and element `Box<[_]>`, an abstract member's parameter-name `Vec`, a record's
field `Vec` ([record.rs](../../src/type_lattice/record.rs)), a deferred return's expression
`String` ([shape.rs](../../src/type_lattice/shape.rs)), and a signature's three `HashMap`s plus
two `Vec`s ([schema.rs](../../src/type_lattice/schema.rs)). Interning a type therefore pays one
or more heap allocations that the run region never sees, and the registry sits on the run frame as
a plain field beside the run region ([run_frame.rs](../../src/machine/execute/run_frame.rs))
rather than in it, so the allocations are invisible to region accounting and are freed one by one
at teardown. The schema's member maps are unordered, so every deterministic reader sorts on read:
`feed_named_types`, `feed_record` and the constructor arm of `component_digest` in
[digest.rs](../../src/type_lattice/digest.rs), `sorted_members` and `sorted_slots` in
[walk/unary.rs](../../src/type_lattice/walk/unary.rs), `write_sig_schema` in
[render.rs](../../src/type_lattice/render.rs), and the name collection in `meet_schemas` in
[sig_relations.rs](../../src/type_lattice/sig_relations.rs).

**Acceptance criteria.**

- `TypeNode<'run>` owns no heap payload: union members, shape elements, quantifier names,
  parameter names, record fields and deferred-return text are `&'run` slices and `&'run str`
  bumped into the run region through `BumpAllocator::slice_from_iter` and `BumpAllocator::text`.
- A signature's member tables and a sealed constructor's variant table are symbol-sorted `&'run`
  slices, and no reader in `src/type_lattice` sorts a member table.
- The node table is a `hashbrown` map built over the run region's `Allocator` impl, keyed by
  digest under the identity hasher, so the registry's content is Drop-free and released with the
  region.
- The verdict table is a heap-owned map, as today.
- `TypeRegistry<'run>` reaches every reader through `RunRegistries` and the run frame; `KType`
  carries no lifetime.
- `with_node` copies the `Copy` node out and releases the table borrow before the reading closure
  runs, so reads nest and a reader may intern, as today.
- Every lattice entry point that builds a call-scoped buffer — the unifier's collector cells, a
  union's flatten buffer, the seal's edge and placement tables, the walk context's shadow stack —
  takes a `BumpAllocator<'_>` and builds it there, and a step body passes the step scratch
  allocator from `DecideCtx::scratch()`; no relation owns a `Vec` or `SmallVec` that dies inside
  the call.
- Interning a type performs no global-heap allocation, and a relation performs none: a dhat flat
  sweep over a program that declares signatures, unions and quantified shapes attributes no
  allocation to `src/type_lattice` paths other than the verdict table.
- The design doc's storage description
  ([type-registry.md § Ownership and reclamation](../../design/typing/type-registry.md#ownership-and-reclamation))
  matches the shipped registry.
- The full `cargo test`, the Miri slate, and the seam-equivalence check pass.

**Directions.**

- *Node table — decided.* A `hashbrown::HashMap<TypeDigest, TypeNode<'run>, IdentityBuildHasher,
  &'run Bump>` over the region's raw `Allocator` seam, which
  [bump.rs](../../workgraph/src/witnessed/bump.rs) offers exactly for a mutable collection whose
  backing store should land in the region's chunks. Insert-only, so the growth strands old bucket
  arrays as region garbage the region releases whole.
- *Verdict table — decided.* Stays a heap-owned `HashMap`: it both grows and shrinks — a bound on
  verdict storage is a permissible knob — and a region releases nothing before the run ends.
- *Parameter-name order — open.* `abstract_type_digest` feeds an abstract member's parameter names
  sorted, while `component_digest` feeds a sealed constructor's in declaration order. Decide which
  is identity, then store the names in that order so neither recipe sorts.
- *Pre-seal window schemas — open.* `RelativeSchema` in
  [window.rs](../../src/type_lattice/window.rs) holds the same maps and dies at the seal. Bump
  them into the run region alongside the sealed twins, or leave them heap-owned as
  declarator-local transients.
- *Scratch allocator plumbing — decided.* Threaded as a parameter, not owned by the registry: a
  registry-private scratch bump would need a reset at depth zero of every entry point, an
  invariant no caller can see. The parameter rides the same signatures that gain `'run`.
- *Sequencing against integration — open.* Every external `TypeRegistry` site the
  [integration item](../refactor/type-lattice-integration.md) retargets gains the `'run`
  parameter; shipping this first retargets each site once. Recommended: this item first.
- *Cross-registry transfer — deferred.* With no persistent map, the merge mechanism
  [cross-registry type-content transfer](../type_language/cross-registry-type-content-transfer.md)
  lists lapses; subgraph copy is the mechanism that item decides over.

## Dependencies

Sequence before the integration item where possible (see Directions); no hard edge is recorded,
since integration can also absorb the lifetime.

**Requires:** none — the lattice core and the region bump door are in the tree.

**Unblocks:** none tracked yet.
