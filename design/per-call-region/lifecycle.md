# Region lifecycle: allocation and lift

Which carriers anchor a per-call region, the lift-time anchor decision, the
consumer-pull node-output lift, and the `alloc_object` cycle gate. Part of the
[per-call region protocol](README.md).

## Carriers

The lifecycle anchor is a `Rc<FrameStorage>`, not a `Rc<CallFrame>`.
`CallFrame` is a thin shell over a refcounted [`FrameStorage`](../../src/machine/core/arena.rs)
— the per-call `KoanRegion` plus the `outer` link that keeps the
lexical-ancestor frames' storage alive. An escaping value pins the
*storage*, leaving the shell uniquely owned so TCO reuse can reset it
(see [§ TCO frame reuse](frames.md#tco-frame-reuse)).

Three `KObject` variants embed an `Option<Rc<FrameStorage>>` lifecycle
anchor:

- `KObject::KFunction(&'a KFunction<'a>, Option<Rc<FrameStorage>>)` — a
  closure value. Anchor is `Some(_)` when the captured definition
  scope lives in a per-call region, `None` when it lives in run-root.
- `KObject::KFuture(KFuture<'a>, Option<Rc<FrameStorage>>)` — a future
  value. The `KFuture` embeds `&KFunction`, a bundle, and a parsed
  `KExpression` whose `Future(&KObject)` parts can independently
  borrow into per-call storage; the anchor pins the per-call region
  alive when any of those borrows points there.
- `KType::Module { module, frame }` (in the value channel's `Type` arm) — a
  first-class module value. `frame` is the per-call `Rc<FrameStorage>`
  of the functor call that minted the module; `None` for top-level
  `MODULE` declarations.

A fourth participant lives on `FrameStorage` itself: `outer:
Option<Rc<FrameStorage>>` chains the parent per-call frame's storage
when a builtin-built frame's child scope's `outer` points into per-call
memory (MATCH / TRY / EVAL / MODULE under a functor call). The two
anchor positions are distinct: the `KObject` anchor keeps the region
alive for an *escaped value*; `outer` keeps it alive for an
*outer-scope lookup* the new frame's child scope performs at run time.

Future carriers that need to extend the lifetime of a per-call region
join the list by growing the same `Option<Rc<FrameStorage>>` field.

## Lift-time anchor decision

`lift_kobject` runs when a per-call value is extracted into a
destination region — typically a closure returned from its defining
frame, a module value flowing out of a functor body, or a future
referencing per-call state. Per carrier:

- **`KFunction`.** Compare `f.captured_scope().region` to the dying
  frame's region pointer. Match → clone the dying frame's `Rc` onto the
  lifted value; mismatch → no `Rc`.
- **`Type`-arm `KType::Module`** (lifted by `lift_ktype`, not `lift_kobject`).
  Compare `m.child_scope().region` to the dying frame's region pointer; same rule.
- **`KFuture`.** Run a targeted membership walk
  (`kfuture_borrows_dying_region`) that asks the dying region's
  `owns_object` side-table whether each embedded `Future(&KObject)`
  borrow points into it, recursing through nested expressions,
  list/dict literals, and bundle arg values; the embedded function
  reference is checked via the same captured-scope-region equality test
  the `KFunction` arm uses. Anchor only fires when at least one
  descendant actually borrows into the dying region. `KoanRegion`
  records every allocated `KObject`'s stable address (typed-arena
  allocations don't move) in an addresses-only `Vec<usize>` so the
  membership query is a single linear scan with no deref or borrow.

Composite variants (`List`, `Dict`) recurse with a `needs_lift`
short-circuit: when no descendant needs anchoring, the existing
`Rc<Vec>` / `Rc<HashMap>` is cloned in place rather than rebuilt.
Koan's collection-immutability contract is what makes the structural
sharing safe.

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
  outlives the reused/freed producer frame. A non-coarsened terminal stays in
  the producer frame. The bare `NodeLift` hook is thereby reusable for any
  delivery edge.

Because `KObject` / `Carried` / `Scope` are invariant in their lifetime, none
of these transitions can be a coercion — each cross-frame move is a genuine
`NodeLift` copy (or the held-Rc re-exposure at storage). Two audited
lifetime-reattach primitives in
[outcome.rs](../../src/machine/execute/outcome.rs) remain: `deps_at_step`
re-anchors consumer-pull dep terminals to the cart-witnessed lifetime the
continuation runs at, and `pin_carried_to_run` re-anchors a node read up to
`'run` for the run-global root drain. (The single-lifetime `Outcome` makes the
former up/down decide-surface bridges unnecessary — the splice slot and dep
value share one lifetime.) They are pinned
in the Miri slate by `tail_call_stamps_result_against_first_callers_return_contract`.

### Fast path

If a dying region allocated zero `KFunction`s (`functions_is_empty`),
no descendant `&KFunction` can point into it, and `lift_kobject`
collapses to a plain `deep_clone`. The gate is sufficient *because*
KFutures do not escape as values today: every borrow into the dying
region that the slow path checks (KFunction captured-scope,
KFuture-embedded function ref and parsed-expression
`Future(&KObject)` parts, Module child-scope) traces back to a
KFunction, so "no KFunction allocated here" implies "no descendant
borrows into here." Once KFutures become first-class values that can
ride through `Future(&KObject)` parts independently of any KFunction,
the gate must add a no-unanchored-KFuture-descendant clause; the slow
path's KFuture arm already carries the membership-walk machinery the
fast path would defer to.

## Cycle gate on `alloc_object`

The anchor mechanism creates a self-referential shape if a composite
carrying an escaping closure is re-allocated into the same per-call
region it already anchors to: the region's storage holds the composite,
the composite holds the `Rc<FrameStorage>`, and the `Rc` holds the region.
Neither side can drop. The case shows up when a body returns a
List / Dict / Tagged / Struct holding a closure — the lift-on-return
machinery attaches the per-call frame's `Rc` to the closure, then a
re-allocation of the composite (via `value_pass`, a dep-finish, etc.)
lands the composite back in the per-call region.

`KoanRegion` carries an `escape: Option<*const KoanRegion>` set by
`CallFrame::new` to the outer scope's region address. `alloc_object`
walks the incoming value's composite tree (`obj_anchors_to`, mirroring
`KObject::deep_clone`'s shape) and, on finding any `Rc<FrameStorage>`
whose `region()` is `self`, redirects the allocation up to the escape
region — where the same `Rc` is no longer self-referential. The
redirect is a single `Option`-check on every per-call `alloc_object`;
run-root has `escape: None` and short-circuits, since the
`Rc<FrameStorage>` shapes the gate looks for can only point at per-call
regions by construction. The escape pointer is stable for the per-call
region's life because `CallFrame::new` heap-pins the outer region via
`Rc`, and the outer always outlives this inner per the lexical-scoping
invariant.

`alloc_object` is one of the named safe wrappers — alongside `alloc_ktype`,
`alloc_function`, `alloc_scope`, `alloc_module`, `alloc_signature`, and
`alloc_operator_group` — that route a single `alloc` engine where the gate
lives. The engine and its `unsafe` erase-store machinery live generically in
the `Region<W>` substrate (`src/machine/core/region.rs`), which
names no Koan type; `KoanRegion` is the Koan instantiation
`Region<KoanStorageProfile>`, with the per-family policy supplied by `Stored`
impls in `core::region`. Every family implements `Stored`, and the engine runs
the gate once for all of them. `KObject` and `KType` answer `anchors_to` by
walking their composite tree; the families that cannot hold a self-targeting
`Rc<FrameStorage>` — `KFunction`, `Scope`, `Module`, `Signature`, and
`OperatorGroup` — declare `anchors_to => false`, so the redirect is uniform
across the whole allocation surface. `Stored` is an open in-crate extension
point rather than sealed; unbypassability comes instead from the substrate's
private `storage` field and that single store path — no `&Region` is ever
exposed, so no `Stored` impl can route a value around the redirect.

