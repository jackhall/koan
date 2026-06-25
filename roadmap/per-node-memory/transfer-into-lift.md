# `transfer_into` and closing the lift relocation unsafe

Recast the consumer-pull lift as a borrow-checked copy into the destination region,
retiring the one irreducible value-path `unsafe`.

**Problem.** When a consumer pulls a dep across a dependency edge,
[`lift`](../../src/machine/execute/lift.rs) copies the value into the consumer's region
and re-anchors any surviving borrow through `unsafe { reattach_value::<CarriedFamily>(value)
}` (lift.rs:48) plus `lift_kobject` (lift.rs:66) — the one audited `unsafe` reattach the
shipped witnessed carrier could not remove, because no borrowed witness is held for a value
about to be copied out.

**Acceptance criteria.**

- [`Sealed<T, W>`](sealed-open.md) has `transfer_into`: it relocates the value into the destination
  while keeping each source region the value still reaches alive by that region's frame `Rc`
  (deep-clone is infeasible — a closure may reference anything reachable from its captured scope and
  Koan has no mechanic to compute the copy set), so the carrier is witnessed by the **set** of
  regions the value reaches and re-anchors with no fabricated lifetime.
- The consumer-pull lift routes `transfer_into`; the `reattach_value::<CarriedFamily>` call
  at `lift.rs:48` is deleted, dropping the value-path `unsafe` count by one.
- The `KObject::KFunction` / `KObject::KFuture` per-value `Option<Rc<FrameStorage>>` anchor is
  removed; a lifted value's reached regions are pinned by the carrier's witness set, and
  `Scope::region_owner.upgrade()` still resolves for every reached captured scope.
- The Miri slate (including the lift/drain tests) is green; `cargo test` and
  `cargo clippy --all-targets` clean.

**Directions.**

- *The witness is the set of regions the value reaches — decided.* Deep-clone is off the table: a
  closure can reference anything reachable from its captured scope, and Koan has no reachability
  mechanic to compute a copy set, so each retained source region is kept alive by its frame `Rc`
  rather than copied. A lifted value can reach several regions (the destination allocation plus each
  retained closure's source), so the carrier is witnessed by a *set* of `Rc<FrameStorage>`. This is
  **not** the ancestral `merge` (which keeps one descendant — here the *dying* source); composition
  is set *union*, a member dropped only when another's `outer` chain already pins it. It must **not**
  be collapsed by splicing `src` into `dst`'s `outer`/escape chain, which risks an `src`↔`dst` cycle
  (`FrameStorage.outer` is an owning `Rc`, so when `src`'s ancestry already reaches `dst` the splice
  closes a loop).
- *Hoist-and-remove the per-value anchor — decided.* The witness lives on the carrier, not in the
  value: the `KObject::KFunction` / `KObject::KFuture` `Option<Rc<FrameStorage>>` liveness anchor is
  removed, and the regions a lifted value reaches are pinned by the carrier's witness set, collected
  during `lift`'s structural walk — which every independently-stored value passes through (a consumer
  pull, or the drain boundary at `interpret.rs`; nothing reaches a slot or a binding walk-free, and a
  slot value is held by its co-stored producer frame whose `outer` chain covers ancestors until then).
  Safe because the anchor is **pure liveness**: production never dereferences it (only `is_some()` /
  `clone()`), and a closure reaches its captured scope structurally through `KFunction::captured` /
  `captured_scope` and its region owner through `Scope::region_owner` (a `Weak`), never through the
  anchor — a separation `Witnessed<T, W>` preserves (`T` holds the structural scope reference, `W`
  the liveness set). The set must include **every** region a reached captured scope lives in, so
  `region_owner.upgrade()` still resolves; this subsumes lift's `existing.is_some()` re-anchor gate.
  At this item's landing the walk still assembles the set — only the pilot families are inverted; once
  [`alloc_object`](alloc-object-witnessed.md) / [`alloc_ktype`](alloc-ktype-witnessed.md) finish the
  inversion, every reached region is already named by the carrier's witness set (folded in by `merge`
  at construction), read off the carrier, retiring the structural walk entirely.
- *Set representation — open.* The regions form a *tree* (a closure over closures branches lineages),
  flattened to the set by the walk; `SmallVec<[Rc<FrameStorage>; 1]>` inline (a singleton in the
  common single-lineage case), deduplicated by region pointer with `outer`-chain subsumption dropping
  a member another already pins.
- *Independent of `attach`, sequenced after the unification — decided.* `transfer_into` is a
  relocation seam, not an access seam — it shares only the `Sealed` type with
  [externally-witnessed-attach](externally-witnessed-attach.md) and does not use `attach`. It does
  pin with the unified set witness from
  [alloc-witness-plumbing](alloc-witness-plumbing.md), so it sequences after that rather than
  directly after `sealed-open`.

## Dependencies

**Requires:**

- [Sealed node-storage carrier and `open`](sealed-open.md) — the `Sealed` type this adds
  `transfer_into` to.
- [Production witness impls and the `alloc` witness plumbing](alloc-witness-plumbing.md) — the
  unified set witness this pins the relocated value with.

**Unblocks:** none.
