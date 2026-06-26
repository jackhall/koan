# `alloc_object` returns `Witnessed`

Migrate the object allocation family onto `yoke`, so every `KObject` born in a per-call region
comes back already bundled with its owning frame's witness.

**Problem.** [`region.alloc_object`](../../src/machine/core/arena.rs) (~25 call sites) returns a
bare `&'a KObject` that is not witnessed at all: the co-location invariant — that the witness pins
*this* value's references — stays implicit in the region machinery, and a transitional
`Witnessed::new` bundle would assert it in prose rather than guarantee it by construction, even
though the [`Witnessed::yoke`](../../src/witnessed.rs) / `merge` constructors and the production
witness plumbing now exist. The regions such an object reaches are named only at a node boundary, by
[transfer-into-lift](transfer-into-lift.md)'s structural walk, rather than folded onto its carrier
at construction.

**Acceptance criteria.**

- `alloc_object` returns a `KObject` bundled with its owning frame's witness, the object built
  inside the witness closure — region-pure parts via `yoke`, a referenced region-resident value (a
  list/dict element, a captured scope) folded in via `merge` against its carrier — so a
  region-resident object is born co-located by construction.
- The object family carries no `Witnessed::new`: a site referencing another witnessed value merges
  it rather than re-asserting co-location in prose.
- A lifted object's reached regions are read off its carrier's witness set, retiring the object arm
  of [transfer-into-lift](transfer-into-lift.md)'s structural walk.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Reuses the plumbing — decided.* The owning-`Rc` threading and `WitnessRegion` /
  `MergeWitness` impls land in [alloc-witness-plumbing](alloc-witness-plumbing.md); this item is
  the object-family conversion over that foundation.
- *Construction inversion, not post-hoc bundling — decided.* The object is built inside the witness
  closure (`yoke` for region-pure parts, `merge` for a referenced witnessed value), not bundled
  after the fact; a `for<'b>` closure cannot accept an already-built `KObject<'a>`. See
  [alloc-witness-plumbing](alloc-witness-plumbing.md).
- *`alloc_function` rides this channel — decided.* A function value is a `KObject::KFunction`, and a
  closure capturing its defining scope mints a self-witnessed scope operand from the frame `Rc` it
  already holds and `merge`s it (the foreign `&'a` borrow a `yoke` closure rejects). So the
  ~3-site `alloc_function` inversion rides the same value-channel shift as the object family — folded
  into this item or a sibling follow-on, settled when the channel below is scoped — carrying no
  `Witnessed::new` either.
- *The within-node value channel must carry the witness set — open.* For `alloc_object`'s `merge` to
  have a carrier operand, a referenced region-resident value must arrive as a carrier, so the
  bind / `Carried` / `KObject` path has to thread the `FrameSet` rather than a bare `&'a` plus the
  per-value `Option<Rc<FrameStorage>>` anchor [transfer-into-lift](transfer-into-lift.md) retires.
  Whether that is a full carrier channel or a lighter set-only channel — and how it meets the
  [value-read](value-reads-to-open.md) side — is unsettled. Recommended: settle it before scheduling
  this item; the construction inversion has no `merge` operands until the channel carries them.

## Dependencies

**Requires:**

- [Production witness impls and the `alloc` witness plumbing](alloc-witness-plumbing.md) —
  supplies the threaded `Rc` and production witness impls this family conversion needs.
- [`transfer_into` and closing the lift relocation unsafe](transfer-into-lift.md) — lands the
  per-carrier witness set and the structural walk this inversion folds into and retires.

**Unblocks:** none.
