# Production witness impls and the `alloc` witness plumbing

Land the production `FrameSet` set-witness, thread the owning frame `Rc` through the
`alloc_object` / `alloc_ktype` surface, and delete the stored cycle-gate escape that leaks.

**Problem.** The [`region.alloc_*`](../../src/machine/core/arena.rs) surface hands back a bare
`&'a T` holding only `&KoanRegion`, with no handle to the owning `Rc<FrameStorage>`. To keep the
cycle-gate redirect target reachable, [`Region`](../../src/witnessed/region.rs) stores it as a
`FrameRegionPin` escape field (`34343691`, added to drop region.rs's last `unsafe`) — and that
stored owning `Rc` is a **live leak**: an escaped closure pins its frame's `FrameStorage`, whose
region owns the parent's `FrameStorage` back through the escape, so
`parent → escaped closure → Rc<FrameStorage_child> → region.escape → parent` never drops. The full
Miri slate reports it as a 1378-allocation process-exit leak (a native `Rc::strong_count` check
confirms a real cycle, not a Miri artifact). The cycle gate fires only for the self-anchoring
families — `alloc_object` / `alloc_ktype` (`anchors_to` is true; `alloc_function` / `alloc_scope`
never redirect) — so the field can only leave `Region` once those two families take the redirect
target as a parameter instead.

**Acceptance criteria.**

- The region-owner witness is `Rc<FrameStorage>` carrying production `WitnessRegion` / `MergeWitness`
  impls whose composition walks the real `outer` ancestor chain, and the carried witness is a *set*
  of it — [`FrameSet`](../../src/machine/core/arena.rs), composing by union with `outer`-chain
  subsumption. The result slot (a `FrameSet`) and the scope handle (an `Rc<FrameStorage>` pin) witness
  on that one region-owner type, so a value-carrier and a scope-carrier `merge` by union.
- `alloc_object` and `alloc_ktype` name their cycle-gate redirect target as a parameter — the owning
  `Rc<FrameStorage>` (the witness a later `yoke` mints from) — and `Region` holds no `FrameRegionPin`
  escape field.
- An escaped-closure run leaves the run-root `FrameStorage` at zero strong refs once the runtime
  drops (a native `Rc::strong_count` check), and the full Miri slate clears the 1378-allocation
  process-exit leak it reports today.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Unify the witness on `Rc<FrameStorage>` region-sets — decided.* The result slot witnesses with
  `Rc<FrameStorage>` like the scope handle (it owns the region, escaping-value-pins-storage is
  TCO-neutral), not the former `Rc<CallFrame>`. Because a value can reach several regions and
  deep-clone is infeasible (see [transfer-into-lift](transfer-into-lift.md)), the carried `W` is a
  *set* of `Rc<FrameStorage>` — a singleton for a single-region carrier (a scope, a same-region
  value), larger for a multi-region value — so the binary pick-descendant `merge` generalizes to set
  union with `outer`-chain subsumption. One global decision, landed here.
- *Owning-`Rc` plumbing — decided (parameter).* `alloc_object` / `alloc_ktype` take the owning
  `Rc<FrameStorage>` (the cycle-gate redirect target) as a parameter; `Region` does **not** keep a
  back-reference to its frame. Threading both gate families here removes the stored `FrameRegionPin`
  escape outright — the escaped-closure back-edge through `Region` is gone, clearing the leak — and
  hands those families the witness a later `yoke` will mint from. The non-gate families
  (`alloc_function` / `alloc_scope` / `alloc_module` / …) never redirect, so they pass no redirect
  target.
- *The family inversions live downstream, not here — decided.* The `alloc` surface's consumers bind
  the result at `&'a` — a value used *within* the region borrow (`DepPlacement::InScope` for a body
  scope at [`runtime.rs`](../../src/machine/execute/runtime.rs); `KObject::KFunction` for a function
  value), where the `&'a region` borrow already witnesses it and the liveness rides a separate
  per-value `Option<Rc<FrameStorage>>` anchor. A `Witnessed` carrier earns its keep only where the
  value crosses a node boundary — the result slot (landed) and the consumer-pull lift
  ([transfer-into-lift](transfer-into-lift.md)) — so the `yoke` / `merge` construction-inversion
  rides the family items there, gated on the lift walk it retires: `alloc_object` / `alloc_ktype`
  invert in their own items, `alloc_function` folds into the object value channel
  ([alloc-object-witnessed](alloc-object-witnessed.md)), and `alloc_scope`'s lexical body scopes stay
  bare `&'a` (consumed within the step by `enter_block`, never crossing a boundary). This item lands
  the plumbing and the leak fix; the end-to-end `yoke` / `merge` proof is downstream.

## Dependencies

**Requires:**

- [FrameStorage self-reference removal](framestorage-self-reference.md) — the restructure that
  gives the production bundle site a witness handle to the value's owning frame.

**Unblocks:**

- [`transfer_into` and closing the lift relocation unsafe](transfer-into-lift.md) — the unified set
  witness its hoist-and-remove relocation pins with; the first production `merge` / union user.
- [`alloc_object` returns `Witnessed`](alloc-object-witnessed.md) — reuses the plumbing and impls.
- [`alloc_ktype` returns `Witnessed`](alloc-ktype-witnessed.md) — reuses the plumbing and impls.
