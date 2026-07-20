# Interned type content behind Copy handles

Make `KType` a `Copy` handle into a run-frame-owned graph that owns all type content.
Final item of the arc landing
[design/typing/type-registry.md](../../design/typing/type-registry.md); it builds on
the shipped run-frame registry
([`registry.rs`](../../src/machine/model/types/registry.rs)) and the lifetime-free
`KType` ([`ktype.rs`](../../src/machine/model/types/ktype.rs)).

**Problem.** `KType` owns its structural content — `Box`/`Vec` children and
`Rc<RecursiveSet>` transport — so cloning a type walks and re-allocates that
structure, and the same type built repeatedly (each `LIST OF Number` elaboration,
each record field-type memo) re-materializes identical content. Content digests
already give every type a value identity
([type-identity.md](../../design/typing/type-identity.md)), but the digest rides
alongside the owned structure rather than standing in for it. The cost shows on
several surfaces:

- `RecursiveSet` rides every `SetRef` as `Rc` content transport, and `SetLocal`
  exists solely to keep sibling references from forming `Rc` cycles
  ([`recursive_set.rs`](../../src/machine/model/types/recursive_set.rs)).
- Hot paths re-clone types that already exist: the record field-type memo
  (`Box<Record<KType>>` on
  [`KObject::Record`](../../src/machine/model/values/kobject.rs)) deep-clones on
  every `.ktype()` and `deep_clone`; `region.alloc_ktype(kt.clone())` re-allocates
  already-materialized types on the resolve/bind paths; lift clones
  element/key/value types wholesale
  ([`lift.rs`](../../src/machine/execute/lift.rs)).
- A pre-seal `SetRef` digests off its `Rc` pointer, so a guard on the verdict
  registry must keep non-content digests out of the cache.

**Acceptance criteria.**

- `KType` is a `Copy` handle — the type's content digest and nothing else; cloning a
  type copies the handle without touching content.
- All structural type content lives in the digest-keyed registry graph owned by the
  run frame and dropped with it; a node stores its variant tag plus labeled child
  handles (the composition edges *are* the content), and building the same content
  twice in a run dedups to one node. Content nodes and verdict edges ride the one
  registry.
- `RecursiveSet` is deleted: members are individual graph nodes whose schemas hold
  absolute (cyclic) member handles; the `SetLocal` and `RecursiveRef` variants and
  the `Rc` transport are gone, the set-relative sibling encoding surviving only as
  an interned relative node kind confined to window elaboration and the SCC-digest
  recipe; the pre-seal window is a scope-carried record of membership and fills, so
  nothing pointer-transient ever digests and the verdict-recording digest guard is
  deleted.
- A member's identity is `(digest of its own strongly-connected component, index)`:
  seal extracts sibling references, partitions the window's members into SCCs, and
  digests the condensation bottom-up — each component presented canonically in
  member-name order, intra-component references as relative indices, references
  outside the component folding the referent's finished digest as external content.
- The record field-type memo, the `alloc_ktype(kt.clone())` re-allocation sites, and
  the lift path's type clones all reduce to handle copies; the `alloc_ktype` family
  is deleted.
- Singleton digest values are unchanged — every standalone `NEWTYPE`/`UNION`/opaque
  mint is a singleton SCC whose canonical presentation reproduces today's set recipe
  byte-for-byte, so every singleton pin and relation in the digest suite holds.
  Multi-member group digests move to the per-SCC recipe, with tests pinning the new
  invariants: an unreferenced co-declared member does not perturb a sibling's digest,
  a non-recursive member unifies with its standalone twin, and member declaration
  order is immaterial.
- `KType::Unresolved` no longer exists: the synchronous bind seam carries an
  unresolved bare type name as its `TypeIdentifier` on a dedicated `Held`/`Carried`
  arm, consumed by the park-capable resolver, so no type handle can denote an
  unresolved name.
- `seed_builtins` ([builtins.rs](../../src/builtins.rs)) consumes the run frame's
  registry it already receives: the `types` operand is threaded into every
  per-module `register(scope, types)` so the builtins' own `KType` construction
  interns against the run's graph.

**Directions.**

- *The graph is the content — decided.* The registry is a multigraph: composition
  edges, labeled by child slot (element / key / value / field name / param name /
  return / member index), replace today's `Box`/`Vec` ownership rather than indexing
  over it; subtype-verdict edges ride the same structure.
- *Handle shape: the digest alone — decided.* A handle is exactly the type's content
  digest: no index, no pointer, no registry reference. Nodes are stored in a map
  keyed by digest, so a handle is also the lookup key for its node; a digest is
  already a uniformly distributed hash, so the map uses it directly as the hash value
  (an identity hasher) and a lookup costs about what an array index would. Because a
  handle records nothing about the registry that minted it, the same handle names the
  same type in every registry.
- *Leaf handles as constants — decided.* Leaf digests are pure functions of nothing,
  so they are hardcoded constants guarded by a test asserting each equals its freshly
  computed node digest; primitive values' `.ktype()` reads stay context-free, and no
  lazy statics or globals appear. The constant set includes every
  pure-function-of-nothing type `from_name` lowers to — the five `OfKind` values,
  `List<Any>`, `Dict<Any, Any>`, and the empty signature — so the synchronous bind
  seam stays context-free.
- *Relative sibling nodes over a builder overlay — decided.* A sibling reference
  interns as a relative index node (today's `SetLocal` digest recipe), so window
  elaboration is ordinary interning; the scope-carried window record fixes
  membership, accumulates fills, and at seal digests each SCC of the condensation
  from the relative digests, then interns member nodes with every sibling rebuilt to
  its absolute handle. See
  [type-registry.md § Recursive sets](../../design/typing/type-registry.md#recursive-sets-are-cyclic-subgraphs).
- *Member identity: computed SCC, name-order canonical — decided.* Identity follows
  the reference structure, not the declaration boundary: the declared group has no
  identity role, so co-declared but unreferencing types digest independently. Names
  are unique within a window, so canonicalization is a sort — no iterative
  refinement. Rejected en route: per-member iterated "incomplete digests" (the hash
  sequence never stabilizes, so identity would hang on an arbitrary stopping round).
- *Node storage: persistent map — decided.* Nodes live in a HAMT (`imbl`) keyed by
  digest through an identity hasher; reads clone nodes out of a short borrow (never
  held across an intern), bulk walks may take an O(1) snapshot, and the persistent
  map keeps the cross-thread merge candidate live.
- *Cross-thread transfer — deferred.* Punted to this project's
  [Unplanned work](README.md#unplanned-work) entry (cross-thread type-content
  transfer); it is exercisable only once concurrency ships. This item must not
  foreclose either candidate mechanism recorded there — node storage stays
  reproducible by a subgraph copy, and verdict edges stay separable from content
  nodes.

## Dependencies

The value-side counterpart is
[Region-store record values](../refactor/region-store-records.md), which homes a
record's field substrate in the region; this item owns every type-side clone.
Interning may collapse digest-equal sets freely: no control flow rests on a
member's digest-excluded origin, since `NominalMember` no longer records one.

**Requires:** none — the premise is in place.

**Unblocks:**

- [Module element-type join](module-element-type-join.md) — supplies the
  owned-schema signature nodes a synthesized join result needs.
- [Module bodies announce type groups](../type_language/module-announced-type-groups.md)
  — computed-SCC identity makes announced-set membership identity-neutral, which the
  module-wide announcement needs.
