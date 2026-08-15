# Region lifecycle: allocation and lift

Which frame pins a per-call region, node-output delivery, and how an escaping
value is kept alive. Part of the [per-call region protocol](README.md).

## Carriers

The lifecycle pin is a `Rc<FrameStorage>`, not a `Rc<CallFrame>`.
`CallFrame` is a thin shell over a refcounted [`FrameStorage`](../../src/machine/core/arena.rs)
— the per-call `KoanRegion` plus the `outer` link that keeps the
lexical-ancestor frames' storage alive. An escaping value pins the
*storage*, so the region outlives the shell independently — a `FreshTail` tail
hop drops the shell while the escapee keeps its snapshot
(see [tail-call-optimization.md](../tail-call-optimization.md)).

A value-side reference into a per-call region is a *bare borrow*: a `KObject::KFunction(&'a
KFunction<'a>)` reaches the per-call region that owns its captured
scope only through that reference, and a `KObject::Module(&'a Module<'a>)` reaches its child
scope's region
the same way. None of these carries an owning `Rc<FrameStorage>` on the value. The region such a value
reaches is kept alive by the value's *holder* — a producer slot's `FrameSet` witness while the value
rides the scheduler, and the binding entry's owned pin bundle once the value is bound out of it
(below) — never by an anchor embedded in the value. Because the in-region value strong-owns no frame, no
allocation can close a region↔value cycle, so the allocation engine carries no cycle gate.

`FrameStorage` itself carries `outer: Option<Rc<FrameStorage>>`, which chains the parent per-call
frame's storage when a builtin-built frame's child scope's `outer` points into per-call memory (MATCH
/ TRY / EVAL). The pin is derived inside `CallFrame::new` from the parent scope's own region owner
([`Scope::parent_frame_pin`](../../src/machine/core/scope.rs)), never passed by the builtin. This is
distinct from escaping-value liveness: `outer` keeps a region alive for an *outer-scope lookup* the
new frame's child scope performs at run time.

## Node-output delivery

A node continuation produces its value at the node's own per-call frame
lifetime `'step` ([`Outcome<'step>`](../../src/machine/execute/outcome.rs)), the
single cart-scale lifetime the decide surface carries: the value is born in the producer's frame (a builtin allocates
it there) or arrives as a dep already delivered into that frame. The scheduler
relocates it across each dep edge — never the producer.

- **Delivery at finalize distributes the terminal the moment it exists.** The
  finalize walk adopts it into each edge's destination region, once per
  distinct destination
  ([dag-scheduler.md § Delivery at finalize](../../workgraph/design/dag-scheduler.md#delivery-at-finalize)):
  a copy verdict rebuilds the value there and frees the producer frame at
  finalize; a pin verdict leaves the value resident in the producer frame and
  transfers that frame into the destination's union bundle. The single workload
  `NodeLift` hook
  ([`src/machine/execute/lift.rs`](../../src/machine/execute/lift.rs)) owns the
  `KObject`-invariant copy; the scheduler loop names no `KObject` / `KType`.
- **Consumers receive residents.** When a consumer runs
  ([`run_step`](../../src/machine/execute/run_loop.rs)) each dep is already an
  ordinary resident of its own call region, at the consuming node's lifetime. A
  value delivered to N consumer regions is adopted N times — once per distinct
  destination — and each copy dies with its destination region. One mechanism
  serves parked-then-woken, late-wired, and bare-name-forward consumers alike.
- **Roots deliver to the run region.** A top-level statement result's root edge
  is held by [`run_program`](../../src/machine/execute/runtime/interpret.rs)
  and destined into the run region, so the terminal lands there at finalize;
  the drain boundary reads residents of the run's own region and releases the
  root edges when it is done.
- **Return-contract enforcement is a separate layer** — the
  [`NodeFinalize`](../../src/machine/execute/finalize.rs) workload hook, peer of
  `NodeLift` — run once at producer Done before the pin: it reattaches the
  erased contract against the producer cart, runs the declared-return check, and
  re-stamps the value **in place**, in the producer's own region (a coarsening
  re-tag, e.g. `List<Number>` through `:(LIST OF Any)`, re-allocates there too).
  Declared or not, it seals the
  [`CarrierWitness`](../../src/machine/core/carrier_witness.rs) — the
  reference-only carrier, pinning nothing — **as-is**: there is no Done-boundary
  relocation or sever gate. The producer frame's lifetime is decided by
  delivery at finalize: a copy verdict frees it there, a pin verdict transfers
  it into each destination region's union bundle
  ([reach.md § Retention model](../../workgraph/design/reach.md#retention-model)) —
  so a region-pure and a frame-borrowing terminal alike leave the frame to the
  delivery walk. The bare `NodeLift` hook is thereby reusable for any
  delivery edge.

Because `KObject` / `Carried` / `Scope` are invariant in their lifetime, none
of these transitions can be a coercion — each cross-frame move is a genuine
`NodeLift` copy (or the held-Rc re-exposure at storage). The dep relocation runs inside the
delivery adopt's own fold at the producer's finalize: the fold's brand is where
[`copy_carried`](../../src/machine/execute/lift.rs) copies a copy-verdict value into the
destination region with a plain `'b → 'b` structural alloc — the
spine sharing its `Rc` payloads, a closure / future / module riding its bare borrow — so the dep is
already a resident of the consumer's region when its step starts. There is no
fabricated lifetime and no value-path `unsafe`: the value lands at the destination region's own
lifetime. (The single-lifetime `Outcome` makes the up/down decide-surface bridges unnecessary — the
splice slot and dep value share one lifetime.) The seam is pinned in the Miri slate by
`tail_call_stamps_result_against_first_callers_return_contract` and `functor_application_is_generative`.

## Escaping-value retention

A relocated closure / future / module rides a *bare* borrow into the per-call region that owns its
defining scope. The copy keeps that borrow verbatim — a closure may reference anything reachable from
its captured scope, and Koan has no reachability mechanic to compute a copy set, so the source region
is *kept alive*, not rebuilt. While the value rides a scheduler slot its producer terminal's `FrameSet`
witness pins that region; once it is relocated out of the scheduler — bound into a persistent scope,
spliced into a working expr and re-dispatched, or read out as a top-level result — the producer slot
is gone, so the *consumer* takes over the pin: the binding scope's region union bundle for a bound
value, the new envelope's bundle for a re-dispatched or read-out one.

Both channels carry the regions a relocated value reaches on its delivered
[`Sealed`](../../workgraph/design/witnessed-memory.md#storage-and-access-seal-open-transfer_into) carrier. A **closure /
future** seals its captured-scope reach at construction; a **module value** names its child scope's
own region, composed by
[`Scope::store_module_object`](../../src/machine/core/scope/reach.rs), which owns the union covering
everything its members reach. The embedding or binding site mints that
carrier's reach — `merge` at an `attr` / `FROM` projection,
[`Scope::adopt_for_binding`](../../src/machine/core/scope/reach.rs) at a `let` / user-fn arg / `USING`
bind — and delivery mints a
[`run_program`](../../src/machine/execute/runtime/interpret.rs) root terminal's full reach against
the run frame's own region, that region's union bundle owning the pins, so a value reaching
several regions (a
list of closures, a module over a functor-result region) keeps every one, read straight off its carrier
rather than reconstructed from the value. The description is exact — every reached region is a member —
while the owned bundle drops what it must not hold: the destination's own region (`pins_region`
subsumption plus the self rule), and, at region-lifetime retention, the run root (the eternal rule).
No cycle forms: a
frame's `outer` chain points only toward its lexical ancestor (or `None` at run-root), never back
toward a descendant, so a minting descendant never strong-refs back into the chain that
would close a loop.

The allocation engine therefore needs **no cycle gate**. A stored value holds no owning `Rc` back to
a region, so storing a composite that carries an escaping closure into any region — including the one
the closure's scope lives in — can never close a region↔value back-edge. Nor is there a store engine
to route around: a `Scope` lands in the region's bump like every other family
([memory-model.md § Move-in residence](../memory-model.md#move-in-residence)), built at the
destination's own `'a` where its fields allow and at a `for<'b>` brand where it embeds a foreign
operand. The allocator lives generically in the `Region<W>` substrate
(`workgraph/src/witnessed/region.rs`), names no Koan type, and the only `unsafe` on the path is the
substrate's single audited `retype`, routed once by the crossing door to re-anchor the freshly
bumped **reference**. It stays unbypassable by capability rather than by privacy of a store path:
the region's own `allocator` is `pub(crate)`, so writing requires a `RegionHandle`.

