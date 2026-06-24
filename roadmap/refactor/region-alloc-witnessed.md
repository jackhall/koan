# `region.alloc` returns `Witnessed`

Make every value born in a per-call region come back already bundled with the liveness witness that
pins it, so the co-location-enforcing [`Witnessed::yoke`](../../src/witnessed.rs) constructor is the
*only* way a region-resident value enters the value channel.

**Problem.** The [`region.alloc_*`](../../src/machine/core/arena.rs) surface (arena.rs:216–263) hands
back a bare `&'a T` and holds only `&KoanRegion` — it has no handle to the owning `Rc<FrameStorage>`,
so it cannot bundle the witness. The shipped [`Witnessed::yoke`](../../src/witnessed.rs) /
[`merge`](../../src/witnessed.rs) primitives are *available and proven* but unused in production: the
lone production bundle site, [`node_store::finalize`](../../src/scheduler/node_store.rs), receives its
value already-produced (`run_loop.rs:175`) and falls back to the caller-asserted `Witnessed::new`. So
the co-location invariant still rides as a prose SAFETY note at every production `new` site, exactly
the gap the constructor was built to close — but cannot close until allocation itself yields a witness.

**Acceptance criteria.**

- `region.alloc_*` returns a value bundled with its owning frame's witness, sourced through
  [`Witnessed::yoke`](../../src/witnessed.rs), so a region-resident value is born co-located by
  construction rather than paired with an asserted witness at a downstream `new`.
- `Rc<CallFrame>` (or `Rc<FrameStorage>`) carries production `WitnessRegion` / `MergeWitness` impls —
  `merge_pin` walks the real `outer` ancestor chain — so the stand-in cart used by the constructor's
  Miri proofs is replaced by the live type.
- The owning `Rc` is threaded through the allocation surface so `alloc_*` can name its witness; no
  production `Witnessed::new` site keeps a caller-asserted co-location SAFETY note where `yoke` now
  applies.

**Directions.**

- *Allocation-surface owning-`Rc` plumbing — open.* Whether `alloc_*` takes the owning
  `Rc<FrameStorage>` as a parameter, or `Region` gains a back-reference to its frame. The former keeps
  `Region` free of the cycle; the latter centralizes the handle.
- *Production witness type — decided.* The bundled witness is `Rc<CallFrame>` / `Rc<FrameStorage>`,
  whose `outer` chain pins the region plus its ancestors — the relation the constructor's `merge_pin`
  stand-in already models.

## Dependencies

This is the production migration deferred out of the co-location-enforcing constructor item; it also
leans on the FrameStorage restructure that gives `finalize` a witness handle at the production site.

**Requires:**

- [Co-location-enforcing `Witnessed` constructor](witnessed-colocation-constructor.md) — supplies the
  `yoke` constructor and `WitnessRegion` / `MergeWitness` traits this wires into production.
- [FrameStorage self-reference removal](framestorage-self-reference.md) — the restructure that gives
  the production bundle site a witness handle to the value's owning frame.

**Unblocks:** none.
