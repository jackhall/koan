# One layer, wherever laid

One form for a body's facts decided under one binding, whether the load, the
drain or a frame lays them.

**Problem.** A body's facts settled under a binding are one kind of thing — a
walk over a parent's facts, rooted at the body, laying each unit's record and
the body's closing beside what it leaves open — and they rest in three forms
([the store](../../src/build/README.md#the-store),
[activations](../../src/build/README.md#activations)). The load lays its
records off the shape in program storage, found by site, through
[`LoadStore`](../../src/build/load.rs). A settlement lays a
[`Layer`](../../src/build/layer.rs)'s writes in an arena node's bump inside a
`self_cell` holding an `Rc` of each arena node it reads, found by its parent
and a key digest, and replayed from a [`Stamp`](../../src/scope/stamp.rs)
after an eviction. An activation lays a layer's writes through the
[records copier](../../src/scope/relay.rs) into the frame's region, found by
the [decision memo](../../src/scope/memo.rs). The walk runs over one `Store`
and the run reads through one `Facts` door, but what a walk lays has three
layouts, three lays and three indexes, so moving a decision from one place to
another — what [the project](README.md) does step by step — rewrites the
layout each time.

**Acceptance criteria.**

- A laid layer — its unit records, closing records, residual, closed trees
  and, under a composition, its terms — is one type, laid by one lay and read
  through one door, whether it rests in program storage, an arena node's bump
  or a frame's region.
- Program storage holds a tree of layers: the load's base layer, binding
  nothing, and under it every layer the load lays; the shape, the folds and
  the closed-tree index stay beside the base. A settlement is a layer in an
  arena slot; an activation's decided layers are the same type in the frame's
  region.
- A layer is found the same way wherever it rests: by its base, the body it
  is rooted at and, under them, its binding.
- The counts the next steps size themselves by are read off the books: how
  many settlements a run keyed, under which call sites, and how many of those
  bindings name load content, reported by a test over the example programs.
- Every program runs as before: the resumption property laws
  ([properties](../../src/build/README.md#properties)) hold unchanged.

**Directions.**

- *The index — open.* One table per base keyed by the body's segment and the
  binding's digests, the arena's key without its tree; or a tree of layers,
  each holding its children's table, the arena's tree laid in program
  storage. The second keeps a chain's nesting in the data and is what
  [traced layers at load](traced-layers-at-load.md) indexes per call site.
- *The arena node's holds — open.* A layer references its parent by index
  wherever it rests and only the arena slot keeps counted holds, so the
  borrower count and the pin stay the arena's books and never the layer's.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Traced layers at load](traced-layers-at-load.md) — the load lays a layer
  in program storage in the form a settlement already has.
- [Settlements retired](settlements-retired.md) — an untraced decision moves
  to the activation without changing its form.
