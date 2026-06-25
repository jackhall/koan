# Production witness impls and the `alloc` witness plumbing

Give the production frame type its `WitnessRegion` / `MergeWitness` impls and thread the owning
`Rc` through the allocation surface, then migrate a pilot allocation family onto `yoke`.

**Problem.** The shipped [`Witnessed::yoke`](../../src/witnessed.rs) / `merge` constructors are
proven only against a stand-in cart `Rc` in their Miri tree; no production type carries the
`WitnessRegion` / `MergeWitness` impls. And the [`region.alloc_*`](../../src/machine/core/arena.rs)
surface hands back a bare `&'a T` holding only `&KoanRegion`, with no handle to the owning
`Rc<FrameStorage>`, so it cannot bundle a witness even where `yoke` now applies.

**Acceptance criteria.**

- `Rc<CallFrame>` / `Rc<FrameStorage>` carry production `WitnessRegion` / `MergeWitness` impls
  whose `merge_pin` walks the real `outer` ancestor chain, replacing the constructor's stand-in
  cart in the production path.
- The owning `Rc` is threaded through the allocation surface so an `alloc_*` family can name its
  witness.
- `alloc_function` (~3 sites) and `alloc_scope` (~12 sites) return values bundled through `yoke`,
  proving the plumbing end to end; their downstream `Witnessed::new` co-location SAFETY notes are
  gone.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Owning-`Rc` plumbing — open.* Whether `alloc_*` takes the owning `Rc<FrameStorage>` as a
  parameter, or `Region` gains a back-reference to its frame. The former keeps `Region` free of
  the cycle; the latter centralizes the handle. Recommended: parameter, decided per family in the
  follow-on migrations.
- *Pilot the smallest families — decided.* `alloc_function` / `alloc_scope` are the lowest-volume
  families, so they carry the plumbing proof; the high-volume families follow as their own items.

## Dependencies

**Requires:**

- [FrameStorage self-reference removal](framestorage-self-reference.md) — the restructure that
  gives the production bundle site a witness handle to the value's owning frame.

**Unblocks:**

- [`alloc_object` returns `Witnessed`](alloc-object-witnessed.md) — reuses the plumbing and impls.
- [`alloc_ktype` returns `Witnessed`](alloc-ktype-witnessed.md) — reuses the plumbing and impls.
