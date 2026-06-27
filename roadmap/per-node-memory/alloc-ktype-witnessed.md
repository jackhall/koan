# `alloc_ktype` returns `Witnessed`

Migrate the type allocation family onto `yoke`, so every `KType` born in a per-call region comes
back already bundled with the set of regions it reaches.

**Problem.** [`region.alloc_ktype`](../../src/machine/core/arena.rs) (~38 call sites — the
highest-volume family) returns a bare `&'a KType` that is not witnessed at all; like the object
path, a transitional `Witnessed::new` would assert co-location in prose rather than guarantee it by
construction, even though the `yoke` / `merge` constructors and the production witness plumbing now
exist. The one region-referencing variant — a `KType::Module` naming its child scope — has its reach
named not on its carrier but by the same two transitional mechanisms the object family uses: a
read-out recovery from the module's child-scope `region_owner`
([`reached_frame`](../../src/machine/execute/lift.rs)) and the consumer-frame accumulator
([`FrameStorage.retained`](../../src/machine/core/arena.rs)). `KType::Module` is the **last** user of
both, and the only value whose slot witness does not *already* name its reach: its child scope lives in
a region distinct from the module's producer frame, so — unlike a `KObject::KFunction`, whose defining
frame *is* its producer frame and whose reach the dep-result currency already carries — the uniform
retain at the [`relocate`](../../src/machine/execute/run_loop.rs) cannot drop to the carried set until
`KType::Module`'s construction folds the child-scope frame onto its carrier.

**Acceptance criteria.**

- `alloc_ktype` returns a `KType` bundled with the set of regions it reaches, built inside the
  witness closure — most `KType`s are owned / `Rc`-shared and `yoke` directly, while a
  region-referencing variant (a `KType::Module` naming its child scope) folds in via `merge` against
  that scope's frame — so a region-resident type is born co-located by construction.
- The type family carries no *prose-asserted* `Witnessed::new`: a variant referencing a witnessed
  value merges it rather than pairing an arbitrary value with an arbitrary witness.
- A lifted `KType::Module`'s reached region is read off its carrier's witness set. With its last user
  converted, the read-out `reached_frame` recovery and the `FrameStorage.retained` accumulator are
  both **deleted** — every reached region is named on the carrier itself.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Reuses the shipped substrate — decided.* Built over the production witness impls and the unified
  `FrameSet` set-witness (see
  [memory-model.md § Region lifetime erasure](../../design/memory-model.md#region-lifetime-erasure));
  this item is the type-family conversion.
- *Its own PR, after the object family — decided.* At ~38 sites the `ktype` conversion is its own PR;
  it lands *after* [alloc-object](alloc-object-witnessed.md), reusing the shared dep-result plumbing
  that item lands (the lift handing each finish its deps' witness sets).
- *Construction inversion, not post-hoc bundling — decided.* The type is built inside the witness
  closure; a `for<'b>` closure cannot accept an already-built `KType<'a>`. Most variants `yoke`
  (owned / `Rc` data); a `KType::Module` `merge`s its child-scope carrier.
- *The scope witness rides the type, not `alloc_scope` — decided.* A `KType::Module`'s child scope is
  alloc'd via `alloc_scope`, but the witness that keeps its region alive rides the `KType::Module`
  carrier (the value), not the scope handle: so this inversion is the `KType` value's, and
  `alloc_scope` itself stays bare `&'a`. The merge operand is the module's defining frame
  (`FrameSet::singleton(F)`, `F` recovered as the child scope's `region_owner`), the same shape as
  the object family's captured-scope merge.
- *Completes the `reached_frame` / `FrameStorage.retained` deletion — decided.* The channel decision
  is settled on [alloc-object](alloc-object-witnessed.md): the set is built at construction by
  `yoke` / `merge` (operand recovered via `region_owner`), composed onward by `merge` /
  `transfer_into`, with the dep-result plumbing carrying the set rather than dropping it at the
  `relocate`. The retain at the `relocate` is a *single uniform* call site; once `KType::Module`'s
  carrier names its child-scope reach, every relocated value carries its reach on its dep currency, so
  the call site drops to reading that set and `reached_frame` loses its last caller. `KType::Module` is
  the last value whose slot witness did not already name its reach; converting it leaves `reached_frame`
  and `FrameStorage.retained` with no callers, so both are deleted here.

## Dependencies

**Requires:**

- [`alloc_object` returns `Witnessed`](alloc-object-witnessed.md) — lands the shared dep-result
  plumbing this reuses, and the object-family half of the `reached_frame` / `FrameStorage.retained`
  retirement this completes.

**Unblocks:** none.
