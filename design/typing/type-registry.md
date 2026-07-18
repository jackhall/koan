# The type registry

All type content lives in one **registry**: a graph owned by the run frame, in which
every distinct type is a single interned node and `KType` is a small `Copy` handle
pointing into it. Type structure is owned centrally, never per value: a `KType`
carries no `Box`/`Vec` children, no region pointers, no `Rc` transport, and no
lifetime parameter. Identity is the eager content digest of
[type-identity.md](type-identity.md); this doc describes where content lives and how it
is reached.

## Terms

- **Registry** — the single owner of all type content in a run. A component of the
  scheduler-owned run frame ([`frame.rs`](../../src/machine/core/arena/frame.rs)),
  reached through execution context by reference. Not a global, not a `thread_local!`
  static: it is dropped with its run frame, and nothing outlives it that can name it.
- **Node** — one interned type. A node stores its variant tag, its scalar payload
  (names, `ScopeId`s, a signature's schema shape), and **handles to its child types**.
  Nodes are immutable from the moment they are interned.
- **Handle** — the `KType` value itself, and it is exactly the type's `Copy` content
  digest (see [type-identity.md](type-identity.md)): no index, no pointer, no
  registry reference. Nodes are stored in a map keyed by digest, so a handle is also
  the lookup key for its node. A digest is already a uniformly distributed hash (a
  truncated BLAKE3 output), so the map can use the digest directly as the hash value
  (an identity hasher) — a lookup costs about what an array index would. Nothing in
  a handle is specific to the registry that minted it, so the same handle names the
  same type in every registry.
- **Composition edge** — a labeled parent→child link between nodes: a `List` node's
  `element`, a `Dict` node's `key` and `value`, a `Record` node's edge per field name,
  a `KFunction` node's edge per parameter name plus `return`, a `Union` node's edge
  per member, a signature node's edge per schema member, a recursive-set member's
  edge to a sibling. Composition edges **are** the content — a node stores its
  children as handles, not as owned substructure. There is no second, derived index
  of "what contains what"; the graph is the single copy.
- **Verdict edge** — a memoized subtype outcome between two nodes, labeled
  `(relation, verdict)` where relation is `MoreSpecific` or `SigSatisfies` and the
  verdict is a boolean (negative outcomes are stored edges too). Verdict edges are
  cache, never truth: every one is re-derivable by the structural walk it memoizes.
- **Multigraph** — the registry's shape: the same pair of nodes can be connected by
  composition edges and verdict edges at once; the two kinds never mix meanings.
- **Interning** — the registry operation that turns built content into a handle:
  compute the digest (bottom-up, from child handles' stored digests), look it up, and
  either return the existing node's handle or insert a new node. Building the same
  content twice in a run therefore yields the same node — one allocation, two handles.

## The handle is the type

`KType` is `Copy`. Cloning a type copies the sixteen-byte digest and touches no
content; passing types by value, storing them in records of slots, and memoizing them
on values all cost the same as copying an integer pair. Equality is unchanged from
[type-identity.md](type-identity.md): compare digests, unconditionally. Hashing keys on
the digest. A handle is pure content identity — it records nothing about which
registry minted it — so equality between any two handles is meaningful, and
dereferencing a handle in any registry that has interned its content finds the node.

Dereference — reading a node's variant, walking its children, rendering a name — goes
through the registry reference the execution context carries. Predicates (the
specificity walk, signature satisfaction) take that reference as a parameter; nothing
in the type layer reaches for ambient static state.

## Types hold no region pointers; values hold scopes

The registry owns every node outright, so nodes contain only owned data. The division
of labor is:

- **Values hold scopes.** `Module` and `ModuleSignature` are runtime *values*; they
  keep their captured scope pointers (`child_scope`, `decl_scope`) exactly as any
  closure-like value does, and they live in regions under the ordinary value rules.
- **Types are content.** The *type* extracted from such a value owns what it needs and
  points at nothing: a signature node stores the owned schema (abstract members,
  manifest members, `VAL`-slot types — each member itself a composition edge to a type
  node), the `ScopeId` sig-id used for same-declaration specificity refinement, and
  the diagnostic path string.

There is no `SigSource`: the empty signature, a `SIG`-declared interface, and a
module's self-sig are all the same kind of node — a signature node over its schema.
The empty signature is a node whose schema has no members; an empty interface is an
empty interface, so it and the module-lattice top `:Module` are one type (the digest
identity of [type-identity.md](type-identity.md) already unifies them). Admission is
one rule — *does the subject module's self-sig satisfy this schema?* — memoized as a
verdict edge, with digest equality serving as the same-module fast path.

Because a `SIG` body binds no runtime values (a value `LET` inside a SIG body is
rejected in favor of a `VAL` slot — [modules.md](modules.md)), everything a signature
value can be asked for is in its schema: ATTR over a first-class signature value
answers member and `VAL`-slot lookups from the owned schema, with no scope access.

A consequence worth stating plainly: no type can dangle. The residence audit's
type-side checks (does this type's region pointer escape its region?) have nothing
to police — a handle is data, valid as long as its registry, and its registry
lives as long as the run.

## Recursive sets are cyclic subgraphs

A strongly-connected group of mutually-recursive nominal types interns as one node per
member. Sibling references are ordinary composition edges, and those edges may form
cycles: the registry does not reclaim by refcount, so a cycle is not a leak hazard
and needs no special sibling encoding to break it. A member's identity stays
`(set digest, index-in-set)` folded into one digest, exactly as
[type-identity.md](type-identity.md) specifies — two independently built groups with
the same content intern to the same nodes.

Construction is two-phase, and the registry only ever sees phase two. A **builder**
type accumulates the group while member schemas are still being elaborated (the
pre-seal window — members exist, schemas fill in as each finalizes). At **seal**, the
builder digests the finished group, interns each member as a node, rewrites
intra-group references into composition edges, and hands back handles. Pre-seal state
never enters the registry, so every node's digest is a true content digest and no
insert-time purity guard is needed.

## Verdict edges memoize subtyping

A subtype verdict is a pure function of a `(subject digest, candidate digest,
relation)` key, so once computed it never changes for the life of the registry. The
registry records verdicts as edges between the nodes they relate — the same structure
that holds the content holds the cache warm alongside it. Both relations
(`MoreSpecific`, the strict specificity walk; `SigSatisfies`, module/signature
structural satisfaction) live on the one graph, and both positive and negative
verdicts are recorded.

Because the key is a digest pair, verdict storage granularity is observationally
identical: [`registry.rs`](../../src/machine/model/types/registry.rs) holds the
verdicts of a run in one flat `(subject, candidate, relation) → bool` map behind a
`RefCell`, reached as `&TypeRegistry` through the execution context and threaded as
the final parameter of every memoized predicate. One key shape is not content-derived
and so is never recorded: a `KType` reaching an *unsealed* recursive set digests by
`Rc` pointer address, an address the allocator may reuse. The `MoreSpecific` sites and
the cross-`SIG` specificity site take arbitrary types as keys, so they call
`digest_is_content` to exclude such a position before recording; the module-satisfaction
sites key on self-sig digests, which are minted from a *sealed* self-sig and are
content-derived by construction. Either way no pointer-transient key ever enters the
map, so a lookup needs no guard of its own.

The asymmetry between the two edge kinds is a design invariant:

- **Composition edges are load-bearing and never evicted.** They are the content; a
  live handle's node must remain dereferenceable for the registry's whole life.
- **Verdict edges are droppable at any time.** Dropping one costs a re-walk of the
  structural predicate, never a wrong answer. A bound on verdict storage is a
  permissible tuning knob, never a semantic one.

## Ownership and reclamation

The run frame owns the registry; the registry owns the nodes; handles own nothing.
Within a run the registry is insert-only — interning adds nodes, nothing removes them
— and the whole graph drops with the run frame. There is no eviction of content, no
garbage collection, no refcounting, and no growth that outlives the run. Dedup keeps
the node population at the number of *distinct* types the run builds, which is what
bounds the growth.

## Concurrency

Koan has no concurrency primitives yet; the registry is designed so that adding them
changes ownership arithmetic, not the model. Each future worker thread runs under its
own run frame and therefore owns its own registry — per-thread interning by
construction, with no locks and no shared table. Digests are minted locally and agree
across threads by content, so two registries never need reconciling to agree on
identity. Moving a value between threads means its types' content must land in the
receiving frame's registry. Two candidate mechanisms: copy the value's type nodes
plus everything reachable through their composition edges, skipping any digest the
receiver already holds; or, if node storage is a persistent (immutable) map, merge
the two maps outright, sharing structure instead of copying. Under either mechanism
the handles themselves need no translation — a digest is the same value in both
registries.

## Open work

The registry home and its verdict edges are shipped
([`registry.rs`](../../src/machine/model/types/registry.rs)); the storage model —
everything this doc says about nodes, handles, and composition edges — lands through
three further roadmap items. Until they ship, type content is owned by each `KType`
value (`Box`/`Vec` children, `Rc<RecursiveSet>` transport, `Signature` region pointers
behind `SigSource`), and `KType` carries a lifetime parameter and is `Clone` not
`Copy`.

- [Signature types own their schema](../../roadmap/type_memos/signature-schema-ownership.md)
  — collapses `SigSource` into owned signature content.
- [KType without a lifetime parameter](../../roadmap/type_memos/lifetime-free-ktype.md)
  — deletes the lifetime parameter and the type-side residence machinery.
- [Interned type content behind Copy handles](../../roadmap/type_memos/interned-type-content.md)
  — content nodes, the `Copy` digest handle, and the recursive-set builder. The
  cross-thread transfer mechanics (including whether verdict edges transfer as
  warm cache) are recorded as unplanned work in the
  [type memos project README](../../roadmap/type_memos/README.md) — undecided even
  within this design.
