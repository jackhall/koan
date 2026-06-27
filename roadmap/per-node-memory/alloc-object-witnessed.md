# `alloc_object` returns `Witnessed`

Migrate the object allocation family onto `yoke`, so every `KObject` born in a per-call region
comes back already bundled with the set of regions it reaches.

**Problem.** [`region.alloc_object`](../../src/machine/core/arena.rs) (~25 call sites) returns a
bare `&'a KObject` that is not witnessed at all: the co-location invariant — that the witness pins
*this* value's references — stays implicit in the region machinery, and a transitional
`Witnessed::new` bundle would assert it in prose rather than guarantee it by construction, even
though the [`Witnessed::yoke`](../../src/witnessed.rs) / `merge` constructors and the production
witness plumbing now exist. The regions an object reaches are not named on its carrier at all; two
transitional mechanisms stand in. The consumer-pull lift drops each dep to a bare
[`Carried`](../../src/machine/model/values/carried.rs) at the
[`relocate`](../../src/machine/execute/run_loop.rs) — *discarding* the dep `Sealed` carrier's witness
set, which it held a moment earlier — then re-derives the reach per-value from a surviving
reference's scope `region_owner` ([`reached_frame`](../../src/machine/execute/lift.rs)) and
accumulates it on the consumer frame ([`FrameStorage.retained`](../../src/machine/core/arena.rs)). So
a read-out recovery and a frame-level accumulator together reconstruct, after the fact, a reach the
carrier could have named at construction.

**Acceptance criteria.**

- `alloc_object` returns a `KObject` bundled with the set of regions it reaches, the object built
  inside the witness closure — region-pure parts via `yoke`, a referenced region-resident value (a
  list/dict element, a captured scope) folded in via `merge` against its carrier — so a
  region-resident object is born co-located by construction.
- The object family carries no `Witnessed::new`: a site referencing another witnessed value merges
  it rather than re-asserting co-location in prose.
- A construction finish receives its deps as witnessed carriers, not bare `Carried`: the
  consumer-pull lift hands each dep's witness set through to the construction site so `merge`
  composes it, rather than discarding it at the `relocate`.
- A lifted object's reached regions are read off its carrier's witness set; the object family routes
  neither the read-out `reached_frame` recovery nor the `FrameStorage.retained` accumulator. (Both
  are *deleted* when [`alloc_ktype`](alloc-ktype-witnessed.md) takes `KType::Module` — their last
  user — off them.)
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Reuses the shipped substrate — decided.* The production `WitnessRegion` / `MergeWitness` impls,
  the unified `FrameSet` set-witness, the `transfer_into` relocation verb, and the per-value frame
  anchor's removal shipped (a stored value now holds no owning `Rc` back to a region, so the engine
  needs no cycle gate; see
  [memory-model.md § Region lifetime erasure](../../design/memory-model.md#region-lifetime-erasure));
  this item is the object-family conversion over that foundation.
- *Construction inversion, not post-hoc bundling — decided.* The object is built inside the witness
  closure (`yoke` for region-pure parts, `merge` for a referenced witnessed value), not bundled
  after the fact; a `for<'b>` closure cannot accept an already-built `KObject<'a>`.
- *`alloc_function` rides this item — decided.* A function value is a `KObject::KFunction`, and a
  closure capturing its defining scope `yoke`s with the witness `FrameSet::singleton(F)` for its
  defining frame `F` (recovered as `scope.region_owner()`, which the builder holds). So the ~3-site
  `alloc_function` inversion is part of the object-family conversion, carrying no `Witnessed::new`
  either.
- *The set is built at construction, not recovered after — decided.* `yoke` / `merge` at the alloc
  site builds the reached-region set directly: a closure / module's operand is its captured scope's
  defining frame (`scope.region_owner()`); an aggregate's operands are its elements' carrier sets,
  flattened by `merge` (a closure capturing closures branches into independent lineages — the reach
  is a tree, folded into one set). The `region_owner` walk survives **only** as this leaf
  constructor; the standalone read-out `reached_frame` recovery is deleted.
- *Operands ride the carriers already in hand — no new channel — decided.* The dep `Sealed` carriers
  the consumer-pull lift already opens carry the sets; the plumbing **keeps** them to the
  construction finish instead of discarding them at the `relocate`. Nothing new threads through
  `Carried` / `Held` / `ArgValue`, and scope bindings stay bare — a capturing container `merge`s the
  scope's frame `{F}`, and because `F` *owns* its reached set, holding `{F}` transitively pins
  everything bound into that scope, so the scope needs no per-binding witness. This is the substrate's
  composition law (one wrapper per node): `merge` folds the tree of lineages into one carrier set, so
  the reach is never stacked `Witnessed`-in-`Witnessed` and never duplicated in a side accumulator.
- *`frame.retained` retires together with `reached_frame` — decided.* The accumulator's only job is
  covering the window between the set-dropping `relocate` and the construction that rebuilds the
  reach; once the carrier names the reach end-to-end (the dep set is no longer dropped), it is
  redundant — it was "the per-step over-approximation the alloc inversions retire" (see
  [per-node-memory.md § Transfer](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into)).
- *Lands the shared dep-result plumbing — decided.* The "keep the dep set to the finish" wiring and
  the carrier-reads-its-own-reach read-out boundaries are family-agnostic, so they land here and the
  [type family](alloc-ktype-witnessed.md) builds on them. The value-read side
  ([value reads](value-reads-to-open.md)) is orthogonal: a copy-out read never touches the set, and
  an escaping read re-anchors through `transfer_into`, which now reads the reach off the carrier.

## Dependencies

This item lands the shared dep-result plumbing (the lift hands each finish its deps' witness sets)
that the type family reuses; the substrate it builds on (`yoke` / `merge`, `FrameSet`,
`transfer_into`) is shipped, so it has no roadmap prerequisite.

**Requires:** none — the witness substrate is shipped.

**Unblocks:**

- [`alloc_ktype` returns `Witnessed`](alloc-ktype-witnessed.md) — reuses the dep-result plumbing this
  lands, and completes the `reached_frame` / `FrameStorage.retained` deletion by taking the last
  `KType::Module` user off them.
