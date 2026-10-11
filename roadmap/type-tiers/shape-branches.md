# Shape branches

A branch of the type lattice per sealed `Shares` body, released when its last
live activation returns, so a type such a body builds does not linger in its
host.

**Problem.** A `Shares` callee lays its bytes in its host's region and
interns the types it builds into the host's
[branch](../../lattice/src/types/identity.md#storage-a-root-and-its-branches). A body that views a module and returns a number leaves nothing
reachable when it returns, yet every carrier it minted, and every type built
over one, stays in the host's branch until the host finishes: a host that
calls such a body once per element of a list of modules grows its branch per
element, and a [`Shares` tail hop](../rewrite/README.md#unplanned-work) leaves its
predecessor's types with the host the same way. Over the tutorial's runnable
programs no sealed body placed `Shares` mints, so none lays a branch
(`classes_over_the_tutorial` in
[the class tests](../../src/dispatch/tests/classes.rs)).

**Acceptance criteria.**

- A sealed body placed `Shares` interns its self-contained content — atoms it
  births and composites over them and root types — into a branch on its body
  shape, and the branch is reset when the body's live-activation count reaches
  zero at a release. A test calls such a body repeatedly in non-tail position
  with new content each call and holds the node count constant across calls.
- Content built over a handle the caller passed is interned into the region
  branch, with any shape-branch child relaid there, and a sealed body placed
  `Fresh` lays no shape branch.
- A knot tied over a value whose memo resolves in a shape branch — a closure's
  or a quote's birth, a cyclic component's tie — has that branch merged into
  the branch of the habitat it is laid in, pinned by a test that passes each
  out through a non-porous slot and calls, `EVAL`s or crosses it after the
  body returned.
- The count goes one, two, one across a same-shape tail hop and reaches zero
  at the chain's final return, pinned by a test; a hop between shapes resets
  the predecessor's branch after the successor's first step.
- The shape branch's reattach is the one `unsafe` in koan outside the test
  build; its SAFETY comment cites the three facts in
  [type tiers](../../design/type-tiers.md#the-shape-branch), and a
  compile-fail doctest pins each forbidden relation, `'here: 'shape` and
  `'shape: 'here`.
- The static pass marks the ties that can pay, and a measurement over the
  example programs counts the branch nodes each sealed `Shares` body would
  otherwise leave in its host.
- Every law in [tests/properties.rs](../../lattice/src/types/tests/properties.rs)
  holds unchanged over a chain of four tiers.

**Directions.**

- *Which bodies — decided.* Sealed and placed `Shares`. A sealed body placed
  `Fresh` stays on the region tier: a shape branch cannot reset at a tail hop,
  since the successor's first step interns before the predecessor's release,
  while a recycled region plus the hop's forced copy holds a looping body's
  node count constant; what `Fresh` gives up is warmth across activations.
- *Where the branch lives — decided.* In a record laid beside the body shape
  in program storage, which the shape references, holding the branch slot, the
  activation count and the presizing memo. The slot is erased between steps,
  reattached per step at a brand with `'graph: 'shape: 'scratch` and no
  relation to `'here`, and cleared at the reset.
- *The count — decided.* Per activation, at frame entry and release, in the
  drain; parked activations count, since a parked scratch state may hold
  scratch nodes over shape nodes.
- *Ties — decided.* Merge at the tie, at the sites the static pass marks,
  rather than a leak analysis over the call graph: a closure returned by any
  callee was relaid at its birth, whatever the callee's class. A tie is marked
  where a capture or a member reads a local whose template is self-contained,
  a `Member` capture of such a fellow included. A quote is born over its `$`
  captures as a closure is over its captures, and neither a function type nor
  a code type exposes what its value captures
  ([type tiers](../../design/type-tiers.md#two-classes-of-body)); a knot is copied at each node's own memo,
  so a cyclic component's tie hides nothing behind a member's seen type. Each
  is marked and merged alike.
- *The per-template tier — decided.* The static pass records per value binder
  whether it is atom-born here: its right-hand side a view site in this body,
  or a read of such a binder. A template is self-contained when its run-bound
  positions are projections off atom-born locals and surfaced names whose
  `USING` operand is atom-born, and foreign where it holds a `FOR ALL` name, a
  `:Type` parameter, a hole, a mark, or a projection off a parameter, capture
  or call result. The pass is exact where it says self-contained and says
  foreign wherever unsure: self-contained content in the region costs a relay
  and the early release, while foreign content in the shape tier would dangle
  on a returned value, and the shape writer resolves through the shape branch
  and the root alone, so a foreign child misses at the lay.
- *The reset trigger — open.* Reset at every zero, or at zero only once the
  branch has grown past its memo, so repeated content stays warm. Retention
  only.
- *Scratch over shape — open.* Keeping `'shape: 'scratch` lets a step's
  scratch nodes reference shape nodes and makes the count a soundness
  obligation; dropping it makes scratch relay shape children per step and a
  wrong count a miss everywhere.

## Dependencies

**Requires:** none — it builds on the branch tier
([storage](../../lattice/src/types/identity.md#storage-a-root-and-its-branches)).

**Unblocks:** none.
