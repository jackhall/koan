# Region lifecycle: allocation and lift

Which frame pins a per-call region, the consumer-pull node-output lift, and how a relocated escaping
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
scope only through that reference, and a `KType::Module { module }` reaches its child scope's region
the same way. None of these carries an owning `Rc<FrameStorage>` on the value. The region such a value
reaches is kept alive by the value's *carrier* — a producer slot's `FrameSet` witness while the value
rides the scheduler, and the consumer scope's own arena once the value is bound out of it (below) —
never by an anchor embedded in the value. Because the in-region value strong-owns no frame, no
allocation can close a region↔value cycle, so the allocation engine carries no cycle gate.

`FrameStorage` itself carries `outer: Option<Rc<FrameStorage>>`, which chains the parent per-call
frame's storage when a builtin-built frame's child scope's `outer` points into per-call memory (MATCH
/ TRY / EVAL). The pin is derived inside `CallFrame::new` from the parent scope's own `region_owner`
([`Scope::parent_frame_pin`](../../src/machine/core/scope.rs)), never passed by the builtin. This is
distinct from escaping-value liveness: `outer` keeps a region alive for an *outer-scope lookup* the
new frame's child scope performs at run time.

## Consumer-pull node-output lift

A node continuation produces its value at the node's own per-call frame
lifetime `'step` ([`Outcome<'step>`](../../src/machine/execute/outcome.rs)), the
single cart-scale lifetime the decide surface carries: the value is born in the producer's frame (a builtin allocates
it there) or arrives as a dep already lifted into that frame. The scheduler
relocates it across each dep edge — never the producer.

- **Producer Done keeps the terminal in its own frame.** The producer does
  not lift at Done. Its [`SlotState::Done`](../../src/machine/execute/run_loop.rs)
  co-stores the terminal with the backing `Rc<CallFrame>`, pinning the
  producer frame until the slot is freed — frame death moves from Done to
  free. The stored `'run` view is re-exposed against that held `Rc` (the same
  held-Rc seam as [§ Seed-side re-anchor](scope-handles.md#seed-side-re-anchor)); honest `'step`
  typing rides the continuation in/out and the pull-lift destination, not
  storage. The single workload `NodeLift` hook
  ([`src/machine/execute/lift.rs`](../../src/machine/execute/lift.rs)) owns the
  `KObject`-invariant copy; the scheduler loop names no `KObject` / `KType`.
- **Consumers pull-lift at read.** When a consumer runs
  ([`run_step`](../../src/machine/execute/run_loop.rs)) it lifts each dep
  from the producer's frame into its own call region, promoting the producer's
  output to the consuming node's lifetime. A value read by N consumers is
  lifted N times — once per consumer — and each copy dies with its consumer's
  frame. One mechanism serves parked-then-woken, late-parking, and
  bare-name-forward consumers alike.
- **Roots drain to the run region.** A consumer-less terminal — a top-level
  statement result — has no consumer to pull it, so
  [`run_program`](../../src/machine/execute/runtime/interpret.rs) lifts each into
  the run region at the drain boundary and re-homes the slot, releasing the
  pinned producer frame. The `run_one` test helper reads roots through the
  frame pin instead, so it is not a drain boundary.
- **Return-contract enforcement is a separate layer** — the
  [`NodeFinalize`](../../src/machine/execute/finalize.rs) workload hook, peer of
  `NodeLift` — run once at producer Done before the pin: it reattaches the
  erased contract against the producer cart, runs the declared-return check, and
  (only on a coarsening re-tag, e.g. `List<Number>` through `:(LIST OF Any)`)
  re-allocates the stamped value into the contract's captured-scope region so it
  outlives the reused/freed producer frame. With no declared return it seals the
  [`CarrierWitness`](../../src/machine/core/carrier_witness.rs) — the
  reference-only carrier, pinning nothing — **as-is**: there is no Done-boundary
  sever gate. The producer frame's lifetime is the scheduler's frame-retention
  hold, seeded at finalize and released once every destination has pulled
  (pull-count zero), so a region-pure and a frame-borrowing terminal alike leave
  the frame to retention. The bare `NodeLift` hook is thereby reusable for any
  delivery edge.

Because `KObject` / `Carried` / `Scope` are invariant in their lifetime, none
of these transitions can be a coercion — each cross-frame move is a genuine
`NodeLift` copy (or the held-Rc re-exposure at storage). The consumer-pull dep relocation runs
*in-band* at the run-loop step brand: each dep terminal is read out borrow-bounded, erased into one
slice carrier, opened alongside the continuation, and copied into the consumer `dest` region by
[`copy_carried`](../../src/machine/execute/lift.rs) with a plain `'b → 'b` structural alloc — the
spine sharing its `Rc` payloads, a closure / future / module riding its bare borrow. The
storage-bound drain / forward path wraps the same copy as
[`relocate_terminal`](../../src/machine/execute/runtime.rs) over `Sealed::transfer_into`. There is no
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
is gone, so the *consumer* takes over the pin: the relocated value's own carrier witness, and the
consumer scope's own arena for a bound value.

Both channels carry the regions a relocated value reaches on its delivered
[`Sealed`](../per-node-memory.md#storage-and-access-seal-open-transfer_into) carrier. A **closure /
future** seals its captured-scope reach at construction; a **`KType::Module`** seals its child scope's
home frame and binding-entry reaches the same way, via
[`Scope::reach_of_child`](../../src/machine/core/scope.rs). The embedding or binding site mints that
carrier's reach into its own arena — `merge` at an `attr` / `FROM` projection,
[`Scope::host_reach_of`](../../src/machine/core/scope.rs) at a `let` / user-fn arg / `USING` bind — and
the [`run_program`](../../src/machine/execute/runtime/interpret.rs) root drain mints the rehomed
terminal's full witness set into the run-root scope's own arena, so a value reaching several regions (a
list of closures, a module over a functor-result region) keeps every one, read straight off its carrier
rather than reconstructed from the value. The mint is guarded by `pins_region`, so a region the consumer
or an ancestor already pins is not re-added, and the minted set dedups by region. No cycle forms: a
frame's `outer` chain points only toward its lexical ancestor (or `None` at run-root), never back
toward a descendant, so a minting descendant never strong-refs back into the chain that
would close a loop.

The allocation engine therefore needs **no cycle gate**. A stored value holds no owning `Rc` back to
a region, so storing a composite that carries an escaping closure into any region — including the one
the closure's scope lives in — can never close a region↔value back-edge. The named safe wrappers
(`alloc_object`, `alloc_ktype`, `alloc_function`, `alloc_scope`, `alloc_module`, `alloc_signature`,
`alloc_operator_group`) each route the single [`alloc`](../../workgraph/src/witnessed/region.rs) engine, which
erases the value to `'static`, stores it, and re-anchors the store to `'a` with no redirect step. The
engine lives generically in the `Region<W>` substrate (`workgraph/src/witnessed/region.rs`), names no Koan type,
and carries **no `unsafe`** of its own: its erase-store routes the scheduler's audited
`erase_to_static` and the single audited `retype`. It stays unbypassable by construction — the substrate's
private `storage` bundle and that single store path mean no `Stored` impl can route around the engine.

