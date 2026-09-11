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
- A signature's member tables, a sealed constructor's variant table, and a constructor's
  parameter-name list are symbol-sorted `&'run` slices, and no reader in `src/type_lattice` sorts
  a member table. Rendering spells a signature's members in stored order.
- The node table is a `hashbrown` map built over the run region's `BumpAllocator<'run>` through
  `memory::bump_table`, keyed by digest under the identity hasher, so the registry's content is
  Drop-free and released with the region. The `contains_quantified` / `contains_rigid` probes read
  flags computed at intern and stored beside the node; no second heap memo exists.
- The verdict table is a heap-owned map, as today.
- `TypeRegistry<'run>` is constructed over a `BumpAllocator<'run>` the caller supplies
  (`TypeRegistry::in_region`), and every lattice reader takes `&TypeRegistry<'run>`; `KType`
  carries no lifetime. Hosting the registry on `RunRegistries` and the run frame is the
  [integration item](../refactor/type-lattice-integration.md)'s retarget.
- `with_node` copies the `Copy` node out and releases the table borrow before the reading closure
  runs, so reads nest and a reader may intern, as today.
- Every lattice entry point that builds a call-scoped buffer — the unifier's collector cells, a
  union's flatten buffer, the seal's edge and placement tables, the walk context's shadow stack —
  takes a `BumpAllocator<'_>` and builds it there, and a step body passes the step scratch
  allocator from `DecideCtx::scratch()`; no relation owns a `Vec` or `SmallVec` that dies inside
  the call.
- Interning a type performs no global-heap allocation, and a relation performs none, on the
  success and the failure path alike: a lattice test under the lib-test binary's counting
  allocator pre-sizes the verdict table, then interns a record, a function type, a union, a
  quantified shape, a signature with every member kind, and a sealed recursive group, runs every
  relation over them, and asserts the thread's allocation count is unchanged. (The program-level
  dhat sweep is the integration item's, since no program reaches the lattice before it.)
- The design doc's storage description
  ([type-registry.md § Ownership and reclamation](../../design/typing/type-registry.md#ownership-and-reclamation))
  matches the shipped registry.
- The full `cargo test`, the Miri slate, and the seam-equivalence check pass.

**Directions.**

- *Node table — decided.* A `hashbrown::HashMap<TypeDigest, TypeNode<'run>, IdentityBuildHasher,
  BumpAllocator<'run>>` over the region's raw `Allocator` seam, which
  [bump.rs](../../workgraph/src/witnessed/bump.rs) offers exactly for a mutable collection whose
  backing store should land in the region's chunks; built through `memory::bump_table`, the one
  place the map implementation is spelled. Insert-only, so the growth strands old bucket arrays as
  region garbage the region releases whole.
- *Verdict table — decided.* Stays a heap-owned `HashMap`: it both grows and shrinks — a bound on
  verdict storage is a permissible knob — and a region releases nothing before the run ends.
- *Parameter-name order — decided.* Identity is the name **set**: a constructor's parameter names
  are stored symbol-sorted on both `AbstractType` and a sealed `TypeConstructor` member, and both
  recipes feed them as stored. Expression-shape quantifiers are untouched — they stay positional.
- *Pre-seal window state — decided.* A window's member and binder lists, its relative schemas and
  its sealed group live in the **declaring node's frame storage**: the node that opens a window
  closes it. `RecursiveGroupWindow<'w>` takes a `BumpAllocator<'w>` at construction and names no
  `'run`; the seal writes only the member nodes into the run region.
- *Scratch allocator plumbing — decided.* Threaded as a parameter, not owned by the registry: a
  registry-private scratch bump would need a reset at depth zero of every entry point, an
  invariant no caller can see. The parameter rides the same signatures that gain `'run`.
- *Sequencing against integration — decided.* This item first, lattice-internal. Every external
  `TypeRegistry` site the [integration item](../refactor/type-lattice-integration.md) retargets
  then gains the `'run` parameter and the scratch operand once.
- *Cross-registry transfer — deferred.* With no persistent map, the merge mechanism
  [cross-registry type-content transfer](../type_language/cross-registry-type-content-transfer.md)
  lists lapses; subgraph copy is the mechanism that item decides over.

## Dependencies

Ships before the integration item (see Directions), which then hosts the registry on the run
frame and threads the step scratch.

**Requires:** none — the lattice core and the region bump door are in the tree.

**Unblocks:** none tracked yet.
