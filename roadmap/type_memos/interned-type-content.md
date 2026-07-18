# Interned type content behind Copy handles

Make `KType` a `Copy` handle into a run-frame-owned graph that owns all type content.
Final item of the arc landing
[design/typing/type-registry.md](../../design/typing/type-registry.md); it builds on
the shipped run-frame registry
([`registry.rs`](../../src/machine/model/types/registry.rs)) and assumes a
lifetime-free `KType` ([KType without a lifetime parameter](lifetime-free-ktype.md)).

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
- `RecursiveSet` members are individual graph nodes referencing each other through
  ordinary (cyclic) composition edges; `SetLocal`, `RecursiveRef`, and the `Rc`
  transport are gone; the pre-seal window is carried by a builder type that converts
  to interned nodes at seal, so pre-seal state never enters the registry and the
  verdict-recording digest guard is deleted.
- The record field-type memo, the `alloc_ktype(kt.clone())` re-allocation sites, and
  the lift path's type clones all reduce to handle copies; the `alloc_ktype` family
  is deleted.
- Type digest values are unchanged — the existing digest test suite passes
  unmodified.

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
  lazy statics or globals appear.
- *Builder over interned transients — decided.* Pre-seal recursive-group schemas
  hold an explicit sibling reference form inside the builder (resolved through the
  ambient builder during the elaboration window), never placeholder nodes in the
  registry; at seal the builder computes every member handle first, then interns the
  member nodes with cyclic edges already resolved.
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

**Requires:**

- [KType without a lifetime parameter](lifetime-free-ktype.md) — a `Copy` digest
  handle requires a lifetime-free `KType`.

**Unblocks:**

- [Module element-type join](module-element-type-join.md) — supplies the
  owned-schema signature nodes a synthesized join result needs.
