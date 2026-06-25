# Production witness impls and the `alloc` witness plumbing

Give the production frame type its `WitnessRegion` / `MergeWitness` impls and thread the owning
`Rc` through the allocation surface, then migrate a pilot allocation family onto `yoke`.

**Problem.** The shipped [`Witnessed::yoke`](../../src/witnessed.rs) / `merge` constructors are
proven only against a stand-in cart `Rc` in their Miri tree; no production type carries the
`WitnessRegion` / `MergeWitness` impls. And the [`region.alloc_*`](../../src/machine/core/arena.rs)
surface hands back a bare `&'a T` holding only `&KoanRegion`, with no handle to the owning
`Rc<FrameStorage>`, so it cannot bundle a witness even where an inverted construction would
`yoke` or `merge` one in.

**Acceptance criteria.**

- `Rc<FrameStorage>` is the region-owner witness (see Directions), carrying production
  `WitnessRegion` / `MergeWitness` impls whose composition walks the real `outer` ancestor chain and
  replaces the constructor's stand-in cart; the carried witness is a *set* of it, so the result slot
  and the scope handle witness with the same type and a value-carrier and a scope-carrier `merge` by
  union.
- The owning `Rc` is threaded through the allocation surface so an `alloc_*` family can name its
  witness.
- `alloc_function` (~3 sites) and `alloc_scope` (~12 sites) invert so the value is built *inside*
  the witness closure and the call returns a `Witnessed`: region-pure parts through `yoke`, a
  reference to a pre-existing region-resident value — a captured scope, a child scope's
  `outer`/`root` — through `merge` against the already-witnessed referent (the foreign borrow the
  `for<'b>` brand rejects; `merge_pin` keeps the descendant frame witness, which already pins any
  ancestor region via its `outer` chain). The two pilot families carry no `Witnessed::new`
  afterward, proving the plumbing end to end.
- A scope or function an inverted site references is witnessed *before* that site (the bottom-up
  order), so no foreign `&'a` borrow is captured into a `for<'b>` closure.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *`yoke` for region-pure leaves, `merge` for pre-existing region-resident references — decided.* A
  value referencing another region-resident value (a `Scope`'s parent, a `KFunction`'s capture)
  cannot be `yoke`d — the `for<'b>` brand rejects the foreign borrow — so it is `merge`d against that
  referent's carrier, whether same-region (the common case, `merge_pin` trivial) or an ancestry-pinned
  region (`merge_pin` keeps the descendant frame witness); `yoke` covers only owned / region-derived
  parts.
- *Capturing the externally-witnessed scope mints its merge operand — decided.* The per-call child
  scope ([FrameStorage self-reference removal](framestorage-self-reference.md)) carries no bundled
  witness, so a `KFunction` capturing it has nothing to `merge` against directly; the inverted site
  mints a self-witnessed scope operand from the frame `Rc` it already holds — co-located (the scope
  lives in that frame's region) and distinct from the frame's own external handle. This does not
  regress TCO: `try_reset_for_tail` ([`arena.rs`](../../src/machine/core/arena.rs)) checks the
  `Rc<CallFrame>` shell, while an escaping capture pins `FrameStorage` (a separate `Rc`), so a
  captured frame is kept exactly when it should be.
- *Construction inversion, not post-hoc bundling — decided.* Each site moves its build into the
  witness closure; `region.alloc_*` is not wrapped after the fact, because a `for<'b>` closure
  cannot accept an already-built `T<'a>`. `Witnessed::new` (pairing a built value with an asserted
  co-located witness) is the transitional rung a family rides until it inverts, not a permanent
  bundler.
- *Unify the witness on `Rc<FrameStorage>` region-sets — decided.* Today the result slot witnesses
  with `Rc<CallFrame>` (`W::Cart = CallFrame`, an `Option<Rc<CallFrame>>` carrier) while the scope
  handle witnesses with `Rc<FrameStorage>`. [`Witnessed::merge`](../../src/witnessed.rs) requires
  both operands to share one `W`, and the inversion's flagship case — a `KFunction`-carrier bound
  into a `Scope`-carrier — crosses the two, so they must unify before any cross-family `merge`
  type-checks. The region-owner type unifies on `Rc<FrameStorage>` (it owns the region, the scope
  handle already uses it, escaping-value-pins-storage is TCO-neutral). Because a value can reach
  several regions and deep-clone is infeasible (see [transfer-into-lift](transfer-into-lift.md)),
  the carried `W` is a *set* of `Rc<FrameStorage>` — a singleton for a single-region carrier (a
  scope, a same-region value), larger for a multi-region value — so the shipped binary
  `MergeWitness::merge_pin` (pick-the-descendant) generalizes to set union with `outer`-chain
  subsumption. This replaces `W::Cart = CallFrame` / `Option<Rc<CallFrame>>`; one global decision,
  landed here.
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
- [`transfer_into` and closing the lift relocation unsafe](transfer-into-lift.md) — the unified set
  witness its hoist-and-remove relocation pins with.
