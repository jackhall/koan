# Constructing circular values

The value-language counterpart to `RECURSIVE TYPES`: build a value that refers to
itself or participates in a reference cycle.

**Problem.** A *type* can be cyclic, but a *value* cannot. `RECURSIVE TYPES` co-declares
a group of mutually-recursive nominals as an `Rc`-owned
[`RecursiveSet`](../../src/machine/model/types/recursive_set.rs); an external handle is
[`KType::SetRef`](../../src/machine/model/types/ktype.rs) and an intra-set back-edge is
`SetLocal(usize)` — a bare index that carries no `Rc`, so the set holds no internal
refcount cycle and frees once its last external handle drops. The whole-group handle
[`KType::RecursiveGroup`](../../src/machine/model/types/ktype.rs) exists and is
documented as "reserved for value-language cycle construction," but it is inert in
value dispatch.

Values are acyclic by construction. A constructor's arguments are already-finished
values (the constructor path in
[`constructors.rs`](../../src/machine/execute/dispatch/constructors.rs) materializes a
[`KObject`](../../src/machine/model/values/kobject.rs) only once its parts are done), so
a field cannot point back at a value that does not yet exist. And the region cycle gate
([`obj_anchors_to`](../../src/machine/core/arena.rs), consulted by
[`Region::alloc`](../../workgraph/src/witnessed/region.rs)) actively redirects any allocation
whose value would hold an `Rc` back into its own frame to the escape frame —
specifically to prevent a refcount cycle, which would leak under the refcount-based
reclamation the memory model assumes. So `NEWTYPE Node = :{next :Node}` types fine, yet
no `Node` can be built whose `next` is itself, and two nodes cannot reference each
other.

**Acceptance criteria.**

- A value can refer to itself or participate in a reference cycle (a self-referential
  `Node`; two mutually-referential nodes), constructed through a declared surface.
- The constructed cycle is reclaimed without leaking — the refcount cycle a naive `Rc`
  graph would form is broken — so dropping the last external handle frees the whole
  group.
- Structural operations over a cyclic value terminate: rendering (`summarize`) and
  equality do not recur unboundedly.
- `KType::RecursiveGroup`'s "reserved" status is resolved — either consumed by the
  value-construction surface or retired.

**Directions.**

- *Cycle representation — open.* Mirror the type side: an `Rc`-owned value group whose
  internal back-edges are indices (a value `SetLocal` carrying no `Rc`, so no refcount
  cycle), versus `Weak` back-references, versus a tracing cycle collector. Recommended:
  index-into-group, symmetric with `RecursiveSet`/`SetLocal`, since that machinery and
  its leak-safety argument already exist.
- *Construction surface — open.* How a cyclic value is declared and knotted (a
  self-naming recursive `LET`; an explicit knot-tying form). Surface syntax/semantics —
  enumerate options and decide with the user.
- *Cycle-gate interaction — decided.* The region cycle gate's redirect-on-self-anchor
  behavior is the safety net the construction path must supersede; the chosen
  representation must satisfy the gate without leaking.

## Dependencies

Builds on the shipped `RECURSIVE TYPES` type-cycle machinery (no roadmap prerequisite).
A constructible cycle forces [structural value equality](../refactor/structural-value-equality.md)
and the renderer to be cycle-safe; coordinate so neither hangs on a cyclic value.
Update [design/typing/user-types.md](../../design/typing/user-types.md) and
[design/memory-model.md](../../design/memory-model.md) when it ships.

**Requires:** none — foundation.

**Unblocks:** none tracked yet.
