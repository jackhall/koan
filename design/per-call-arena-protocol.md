# Per-call arena protocol

The contract for [`Rc<CallArena>`](../src/machine/core/arena.rs): which
[`KObject`](../src/machine/model/values/kobject.rs) variants carry a
per-call anchor, how
[`lift_kobject`](../src/machine/execute/lift.rs) decides to attach one,
how the `alloc_object` cycle gate routes self-referential allocations,
how the [scheduler](../src/machine/execute/run_loop.rs) propagates the
active frame, how builtin-built frames chain the call-site frame's
storage through `FrameStorage.outer`, and how the TCO step reuses the
frame shell over a fresh `FrameStorage`.
The participants live in `KObject` (carriers), `arena.rs` (allocation
/ storage), and `Scheduler` (active-frame plumbing); this page is the
single named owner so a reader investigating the protocol lands here
rather than reconstructing it from five docs and ten source files.

## Carriers

The lifecycle anchor is a `Rc<FrameStorage>`, not a `Rc<CallArena>`.
`CallArena` is a thin shell over a refcounted [`FrameStorage`](../src/machine/core/arena.rs)
— the per-call `RuntimeArena` plus the `outer` link that keeps the
lexical-ancestor frames' storage alive. An escaping value pins the
*storage*, leaving the shell uniquely owned so TCO reuse can reset it
(see [§ TCO frame reuse](#tco-frame-reuse)).

Three `KObject` variants embed an `Option<Rc<FrameStorage>>` lifecycle
anchor:

- `KObject::KFunction(&'a KFunction<'a>, Option<Rc<FrameStorage>>)` — a
  closure value. Anchor is `Some(_)` when the captured definition
  scope lives in a per-call arena, `None` when it lives in run-root.
- `KObject::KFuture(KFuture<'a>, Option<Rc<FrameStorage>>)` — a future
  value. The `KFuture` embeds `&KFunction`, a bundle, and a parsed
  `KExpression` whose `Future(&KObject)` parts can independently
  borrow into per-call storage; the anchor pins the per-call arena
  alive when any of those borrows points there.
- `KType::Module { module, frame }` (in the value channel's `Type` arm) — a
  first-class module value. `frame` is the per-call `Rc<FrameStorage>`
  of the functor call that minted the module; `None` for top-level
  `MODULE` declarations.

A fourth participant lives on `FrameStorage` itself: `outer:
Option<Rc<FrameStorage>>` chains the parent per-call frame's storage
when a builtin-built frame's child scope's `outer` points into per-call
memory (MATCH / TRY / EVAL / MODULE under a functor call). The two
anchor positions are distinct: the `KObject` anchor keeps the arena
alive for an *escaped value*; `outer` keeps it alive for an
*outer-scope lookup* the new frame's child scope performs at run time.

Future carriers that need to extend the lifetime of a per-call arena
join the list by growing the same `Option<Rc<FrameStorage>>` field.

## Lift-time anchor decision

`lift_kobject` runs when a per-call value is extracted into a
destination arena — typically a closure returned from its defining
frame, a module value flowing out of a functor body, or a future
referencing per-call state. Per carrier:

- **`KFunction`.** Compare `f.captured_scope().arena` to the dying
  frame's arena pointer. Match → clone the dying frame's `Rc` onto the
  lifted value; mismatch → no `Rc`.
- **`Type`-arm `KType::Module`** (lifted by `lift_ktype`, not `lift_kobject`).
  Compare `m.child_scope().arena` to the dying frame's arena pointer; same rule.
- **`KFuture`.** Run a targeted membership walk
  (`kfuture_borrows_dying_arena`) that asks the dying arena's
  `owns_object` side-table whether each embedded `Future(&KObject)`
  borrow points into it, recursing through nested expressions,
  list/dict literals, and bundle arg values; the embedded function
  reference is checked via the same captured-scope-arena equality test
  the `KFunction` arm uses. Anchor only fires when at least one
  descendant actually borrows into the dying arena. `RuntimeArena`
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
lifetime `'step` ([`Outcome<'step>`](../src/machine/execute/outcome.rs)), the
single cart-scale lifetime the decide surface carries: the value is born in the producer's frame (a builtin allocates
it there) or arrives as a dep already lifted into that frame. The scheduler
relocates it across each dep edge — never the producer.

- **Producer Done keeps the terminal in its own frame.** The producer does
  not lift at Done. Its [`SlotState::Done`](../src/machine/execute/run_loop.rs)
  co-stores the terminal with the backing `Rc<CallArena>`, pinning the
  producer frame until the slot is freed — frame death moves from Done to
  free. The stored `'run` view is re-exposed against that held `Rc` (the same
  held-Rc seam as [§ Seed-side re-anchor](#seed-side-re-anchor)); honest `'step`
  typing rides the continuation in/out and the pull-lift destination, not
  storage. The single workload `NodeLift` hook
  ([`src/machine/execute/lift.rs`](../src/machine/execute/lift.rs)) owns the
  `KObject`-invariant copy; the scheduler loop names no `KObject` / `KType`.
- **Consumers pull-lift at read.** When a consumer runs
  ([`run_step`](../src/machine/execute/run_loop.rs)) it lifts each dep
  from the producer's frame into its own call arena, promoting the producer's
  output to the consuming node's lifetime. A value read by N consumers is
  lifted N times — once per consumer — and each copy dies with its consumer's
  frame. One mechanism serves parked-then-woken, late-parking, and
  bare-name-forward consumers alike.
- **Roots drain to the run arena.** A consumer-less terminal — a top-level
  statement result — has no consumer to pull it, so
  [`run_program`](../src/machine/execute/runtime/interpret.rs) lifts each into
  the run arena at the drain boundary and re-homes the slot, releasing the
  pinned producer frame. The `run_one` test helper reads roots through the
  frame pin instead, so it is not a drain boundary.
- **Return-contract enforcement is a separate layer** — the
  [`NodeFinalize`](../src/machine/execute/finalize.rs) workload hook, peer of
  `NodeLift` — run once at producer Done before the pin: it reattaches the
  erased contract against the producer cart, runs the declared-return check, and
  (only on a coarsening re-tag, e.g. `List<Number>` through `:(LIST OF Any)`)
  re-allocates the stamped value into the contract's captured-scope arena so it
  outlives the reused/freed producer frame. A non-coarsened terminal stays in
  the producer frame. The bare `NodeLift` hook is thereby reusable for any
  delivery edge.

Because `KObject` / `Carried` / `Scope` are invariant in their lifetime, none
of these transitions can be a coercion — each cross-frame move is a genuine
`NodeLift` copy (or the held-Rc re-exposure at storage). Two audited
lifetime-reattach primitives in
[outcome.rs](../src/machine/execute/outcome.rs) remain: `deps_at_step`
re-anchors consumer-pull dep terminals to the cart-witnessed lifetime the
continuation runs at, and `pin_carried_to_run` re-anchors a node read up to
`'run` for the run-global root drain. (The single-lifetime `Outcome` makes the
former up/down decide-surface bridges unnecessary — the splice slot and dep
value share one lifetime.) They are pinned
in the Miri slate by `tail_call_stamps_result_against_first_callers_return_contract`.

### Fast path

If a dying arena allocated zero `KFunction`s (`functions_is_empty`),
no descendant `&KFunction` can point into it, and `lift_kobject`
collapses to a plain `deep_clone`. The gate is sufficient *because*
KFutures do not escape as values today: every borrow into the dying
arena that the slow path checks (KFunction captured-scope,
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
arena it already anchors to: the arena's storage holds the composite,
the composite holds the `Rc<FrameStorage>`, and the `Rc` holds the arena.
Neither side can drop. The case shows up when a body returns a
List / Dict / Tagged / Struct holding a closure — the lift-on-return
machinery attaches the per-call frame's `Rc` to the closure, then a
re-allocation of the composite (via `value_pass`, a dep-finish, etc.)
lands the composite back in the per-call arena.

`RuntimeArena` carries an `escape: Option<*const RuntimeArena>` set by
`CallArena::new` to the outer scope's arena address. `alloc_object`
walks the incoming value's composite tree (`obj_anchors_to`, mirroring
`KObject::deep_clone`'s shape) and, on finding any `Rc<FrameStorage>`
whose `arena()` is `self`, redirects the allocation up to the escape
arena — where the same `Rc` is no longer self-referential. The
redirect is a single `Option`-check on every per-call `alloc_object`;
run-root has `escape: None` and short-circuits, since the
`Rc<FrameStorage>` shapes the gate looks for can only point at per-call
arenas by construction. The escape pointer is stable for the per-call
arena's life because `CallArena::new` heap-pins the outer arena via
`Rc`, and the outer always outlives this inner per the lexical-scoping
invariant.

`alloc_object` is one of the named safe wrappers — alongside `alloc_ktype`,
`alloc_function`, `alloc_scope`, `alloc_module`, `alloc_signature`, and
`alloc_operator_group` — that route a single `alloc` engine where the gate
lives. The engine and its `unsafe` erase-store machinery live generically in
the `StorageFrame<W>` substrate (`src/machine/core/region.rs`), which
names no Koan type; `RuntimeArena` is the Koan instantiation
`StorageFrame<KoanStorageProfile>`, with the per-family policy supplied by `Stored`
impls in `core::arena`. Every family implements `Stored`, and the engine runs
the gate once for all of them. `KObject` and `KType` answer `anchors_to` by
walking their composite tree; the families that cannot hold a self-targeting
`Rc<FrameStorage>` — `KFunction`, `Scope`, `Module`, `Signature`, and
`OperatorGroup` — declare `anchors_to => false`, so the redirect is uniform
across the whole allocation surface. `Stored` is an open in-crate extension
point rather than sealed; unbypassability comes instead from the substrate's
private `storage` field and that single store path — no `&Arena` is ever
exposed, so no `Stored` impl can route a value around the redirect.

## Active-frame propagation

The scheduler exposes the currently running slot's frame to code that
needs to capture it ([builtin-built frame chaining](#outer-frame-chain-for-builtin-built-frames)
below, deferred sub-Dispatch under a per-call frame). Three pieces of
state live on `Scheduler`:

- **`active_frame: Option<Rc<CallArena>>`** — frame of the slot
  currently being executed. Read through
  [`Scheduler::current_frame`](../src/machine/execute/run_loop.rs);
  written only by `enter_slot_step` / `exit_slot_step` (the RAII
  bracket `run_step` wraps each slot step in) and the
  `swap_active_frame` save/restore. An invoke never takes it (tail
  reuse draws from the reserve, below), so within a step it is always
  `Some` — `Node::frame` and `PostStep::prev_frame` are non-optional.
- **`active_reserve: Option<Rc<CallArena>>`** — the slot's reserve
  frame, drained from `Node`'s `Frame::reserve` through
  `enter_slot_step` and consumed by `acquire_tail_frame` (see
  [§ Ping-pong reserve frame](#ping-pong-reserve-frame)).
- **`Scheduler::swap_active_frame(frame) -> Option<Rc<CallArena>>`** —
  installs `frame` as `active_frame` and returns the previous one, for a
  transient save/restore. Used by
  [`KoanRuntime::dispatch_body`](../src/machine/execute/runtime/submit.rs) to
  dispatch a body's non-tail statements under the body frame so each sub-slot
  inherits it as its cart (see
  [typing/functors.md § Deferred return-type elaboration](typing/functors.md#deferred-return-type-elaboration)
  for the per-call type-side bind that motivates it).

`Scheduler::execute` *moves* `node.frame` into `self.active_frame`
(no clone) for the duration of each step. That single-ownership
discipline is what lets the tail-reuse path detect "nothing escaped":
when the just-finished active frame rotates into the slot's reserve and
a later step tries to reuse it, `try_reset_for_tail`'s `Rc::get_mut`
succeeds only at `strong_count == 1` — a clone visible to `strong_count`
(an escaped closure, a sub-Dispatch that cloned `active_frame`) is a
real escape and refuses the reset. Sub-dispatch and dep-finish slots inherit
`active_frame` so they see the right ancestor for their own chaining decisions.

## Outer-frame chain for builtin-built frames

A user-fn call's per-call frame is anchored by lexical scoping: the
new frame's child scope's `outer` is the FN's *captured* scope
(run-root for top-level FNs), which outlives every per-call frame.
Builtins that build their own per-call frame don't always have that
property. The frame-chain `Rc` on `FrameStorage` (`outer:
Option<Rc<FrameStorage>>`) keeps the parent frame's storage alive
whenever the child's `outer` points into per-call memory. The builtin
threads the chain by passing the call-site frame's `storage_rc()` into
`CallArena::new`, which stores it on the new frame's `FrameStorage.outer`.

Each builtin clones `sched.current_frame()` into its `CallArena::new`
call:

- `match_case.rs` — MATCH constructs a frame whose child scope's
  `outer` is the **call-site** scope so free names in the arm body
  resolve against the surrounding call.
- `try_with.rs` — TRY-WITH dispatches each branch under a frame
  chained to the TRY call site so the branch body's free names
  resolve through the surrounding call.
- `eval.rs` — EVAL builds a per-call frame for the evaluated
  expression.
- `module_def.rs` — MODULE captures `sched.current_frame()` so the
  module's child scope chains through the call site (relevant when a
  functor body declares an inner MODULE).

Top-level FN invokes pass `None` to `CallArena::new` (their captured
chain ends in run-root, which outlives the run; no chain is needed and
TCO recursion stays bounded). Field declaration order on `FrameStorage`
is load-bearing: `arena` is declared before `outer`, so the
auto-derived `Drop` tears down this frame's arena *before* releasing
the parent storage Rc — inner pointers die before the outer storage they
may reference. The shell's own field order mirrors this: `storage` drops
before `scope_ptr`, so the arena tears down before the now-dangling
child pointer.

## TCO frame reuse

Each TCO step would otherwise drop the previous slot's `CallArena` and
allocate a fresh one — six typed-arena pools, an
`Rc<RefCell<Vec<usize>>>`, an alloc'd child `Scope`, and the
`Rc<CallArena>` box itself per iteration. `CallArena::try_reset_for_tail`
reuses the shell across iterations: install a fresh `FrameStorage` (a new
empty `RuntimeArena`, `outer` re-linked to the new call's captured
scope), re-allocate the child `Scope` into it. The shell `Rc` and the
slot's `frame` field carry over unchanged; the old `FrameStorage` (and
its arena) drops here *unless* an escaped value still pins it, in which
case that snapshot lives on independently while the shell reuses. The
arena address therefore *changes* across a reset (the fresh
`FrameStorage` is a new heap box) — no code captures an arena pointer
across a reset, and for safe code the borrow checker guarantees it can't
(see the cross-reset capture invariant below).

Two structural invariants make the reset sound:

- **No live shell alias.** `Rc::get_mut` succeeds iff no other
  `Rc<CallArena>` *shell* clone exists. An escaped value pins
  `FrameStorage`, not the shell, so it does **not** foreclose reuse:
  the swap drops the shell's reference to the old storage while the
  escapee's clone keeps that snapshot alive and aliased. Only a
  transient shell clone (a sub-Dispatch slot that cloned the shell
  `Rc`) keeps `strong_count > 1` and refuses, falling through to
  `CallArena::new`. The gate's correctness depends on
  `Scheduler::execute` moving `node.frame` into `self.active_frame`
  for the duration of each step — see [§ Active-frame propagation](#active-frame-propagation).
- **No live external refs into the arena's storage.** By the time TCO
  Replace fires, every sub-Dispatch slot the previous body spawned has
  terminalized and freed, and the slot's `dep_edges` are cleared. The
  only remaining references into the old arena's contents live in the
  slot's own scope, which we're about to rebind. Installing fresh
  storage drops the old contents safely (or hands them to the escapee
  that pinned them).

**Cross-reset arena capture is borrow-checker-enforced for safe code.**
`CallArena::arena()` returns an `&self`-bounded `&RuntimeArena`, while
`try_reset_for_tail` takes `&mut Rc<CallArena>`, so a live arena borrow
cannot span that frame's reset — a captured pointer across the reset is
a compile error, not a discipline the code must remember. The sole
lifetime-erased arena exposure is `with_frame_interior`'s seed-bind
re-exposure (the C0-irreducible site witnessed by the held frame `Rc` —
see [§ Seed-side re-anchor](#seed-side-re-anchor)).

Frame reuse is what makes deep tail recursion truly constant-memory —
both in the scheduler's slot table (the `Tail` rewrite alone) and on
the heap (the reset turns over arena storage in place rather than
allocating per step). The harness acquires the body's frame for the pure
`dispatch::exec::invoke` decide through `Scheduler::acquire_tail_frame(outer)`,
which reuses the slot's **reserve** cart — resetting it in place when uniquely owned —
and otherwise allocates a fresh `CallArena::new`. Reuse draws from the
reserve, never the live active frame, so an invoke never empties the
slot's own cart. A reserve carrying an escaped closure (or any other
clone of its `Rc`) fails `try_reset_for_tail`'s `Rc::get_mut` and falls
through to a fresh frame, preserving snapshot semantics for the escaped
value.

### MATCH frame lifetime under tail recursion

When a user-fn recurses through a `MATCH` arm, the recursive call sits
inside the MATCH-built per-call frame, not the user-fn's own frame.
MATCH clones the user-fn's frame storage Rc onto its own frame's
`FrameStorage.outer`, so the user-fn frame's storage stays alive for the
duration of the arm body — without that chained Rc, the recursive arm
body's `outer` pointer into the dying frame would dangle on TCO Replace.
A reserve whose shell is still aliased fails `try_reset_for_tail`'s
`Rc::get_mut` and falls through to a fresh frame; reuse resumes once the
alias drops.

The bound the `chained_user_fn_tail_calls_reuse_one_slot` and
`match_driven_tail_recursion_completes` tests pin is: the user-fn frame
is alive across exactly one MATCH-arm iteration at a time, and the call
chain collapses to one scheduler slot via the `Tail` rewrite even when a
reset refuses on individual MATCH-arm steps.

## Ping-pong reserve frame

An invoke runs synchronously while the slot's `scope` borrows into the
**active** frame's arena, so that frame's tree-borrows protector is live
across the invoke: resetting the active frame in place mid-step would
deallocate the arena out from under a live borrow. Tail reuse therefore
never touches the active frame — it draws from a **different** frame, two
iterations old, that is past every live protector.

To supply one, the slot carries a per-iteration **reserve frame** in
`Frame::reserve` that ping-pongs across `NodeStep::Replace`:

- **Replace arm in `execute.rs`.** On a new-frame Replace, drop the
  (now two-iterations-old) reserve, rotate the post-step frame into
  the slot's `reserve`, install the new frame as the slot's `cart`.
  First iteration's reserve stays `None`; second iteration fills it;
  iteration 3+ has a reserve to consume.
- **Reserve-consuming `acquire_tail_frame`.** `enter_slot_step` drains
  the slot's `reserve` into `Scheduler::active_reserve`; on the next
  invoke, `acquire_tail_frame` takes it and calls `try_reset_for_tail`.
  Its shell `strong_count` is 1 (only the reserve field held it), so the
  reset lands and the body runs in the reset arena. A value that escaped
  while that frame was the active cart two iterations ago pins the
  *storage*, not the shell, so it doesn't foreclose the reset — its
  snapshot rides the old `FrameStorage` while the shell reuses. Only a
  lingering *shell* clone makes `Rc::get_mut` refuse, and
  `acquire_tail_frame` then allocates fresh instead.

The dispatcher reads the slot's reserve / active-frame state from the
execution layer (see [execution-model.md § The dispatcher / scheduler
boundary](execution-model.md#the-dispatcher--scheduler-boundary)):
`dispatch::exec::invoke` is a pure decide against a `SchedulerView`, and the
harness `apply_outcome` arm acquires the cart via `KoanRuntime::acquire_tail_frame`
before handing it to the decide. The `active_frame` / `active_reserve` state lives
on the driver's ambient context (`KoanRuntime`), not the scheduler — the scheduler
is a pure DAG runtime; the accessor surface is what dispatch sees.

The two-iteration gap is the safety witness: when iteration N consumes
the reserve, the reserve's scope was the active scope on iteration
N-2 and is past every live tree-borrows protector by the time
iteration N's invoke fires. Miri full-slate green on
`recursive_tagged_match_no_uaf` — which exercises exactly this pattern
at every iteration — under `MIRIFLAGS=-Zmiri-tree-borrows` is the
structural confirmation.

Steady-state allocation on the stateful keyworded /
`FunctionValueCall` recursive loop is one `RuntimeArena` per iteration
(the inner arena `try_reset_for_tail` installs); the `CallArena`
shell and its `Rc` reuse across iterations after the first
two-iteration warmup.

## Slot-table scope handle

A scheduler slot stores its scope as a lifetime-free
[`NodeScope`](../src/machine/execute/nodes.rs), not a raw `&'a Scope<'a>`, so the node it sits on
pins no `'run` through its scope. The handle rides a grouped `NodePayload` (the scope handle plus the
node's lexical chain) alongside the slot's frame. Both arms are **cart-witnessed** — re-projected
from the slot's live frame at read, never re-anchored at a free `'run`:

- `Yoked` carries no payload at all: the slot's scope *is* its own per-call cart's scope, re-read
  from the frame at the read boundary. Single-cart, because the slot's own `Frame::cart`
  `Rc<CallArena>` is the sole liveness witness, so there is no second `Rc` clone and no contention
  with `try_reset_for_tail`'s `strong_count == 1` TCO reuse check.
- `YokedChild(ScopePtr<'static>)` holds an erased pointer to a block scope a builtin allocated in a
  cart *ancestor* arena (an `InScope` body — USING / MODULE / SIG / TRY), re-attached at read with a
  borrow bounded by the slot's frame `Rc`, sound because the cart's `FrameStorage.outer` chain pins
  that ancestor arena for as long as the slot holds the cart. It differs from `Yoked` only in that the
  child scope differs from the cart's own scope, so it needs a stored pointer.

The funnel [`resolve_node_scope`](../src/machine/execute/runtime/submit.rs) decides the arm in
order: a pointer test (`std::ptr::eq(active_frame.scope(), scope)`) routes a frame's-own-child slot
to `Yoked`; a walk of the active cart's scope `outer` chain that reaches `scope`'s arena routes a
cart-ancestor block scope to `YokedChild`, erasing the borrow through `ScopePtr::erase_static`; the
frameless top-level run root routes to `Yoked` via the `run_frame` cart that adopts it (the slot's
cart is that `run_frame`). The two residual fall-throughs are `unreachable!` — an instrumented
whole-suite spike confirmed every framed submission resolves to `Yoked` / `YokedChild` and every
frameless one to the run root. Storing an erased handle rather than a live `&'run` keeps the borrow
honest across a TCO `try_reset_for_tail`: nothing persisted points into the reset arena.

The read boundary hands a slot's scope back on demand, not as a stored free `&'run`:
[`reattach_node_scope`](../src/machine/execute/dispatch/ctx.rs) materializes it per use — a
`YokedChild` slot re-attaches its erased `ScopePtr<'static>` through the `unsafe` `reattach_bounded`
(borrow bounded by the frame `Rc`, content lifetime free, sound because the cart pins the ancestor
arena); a `Yoked` slot re-reads from the live
`active_frame` cart via [`CallArena::scope_bounded`](../src/machine/core/arena.rs), a
**witness-bounded** brand whose borrow is capped at the `&Rc<CallArena>` receiver (content `'a`
free, `'a: 'p`). Because the borrow cannot outlive the frame `Rc` it reads from, storing it past
the frame is a compile error rather than a fabrication; `Scope<'a>` invariance rides structurally
on the returned `Scope<'a>`, so the brand needs no separate struct. Bodies / finishes / the
dispatch engine no longer thread a `scope` parameter — they call `current_scope()`; the genuine
run-scope methods (`dispatch_in_scope` / `dispatch_in_scope_with_chain` /
`enter_block`) keep their `&'a Scope` argument.

The post-step loop in `Scheduler::execute` reads the just-finished step's scope through a
`PostStep` token returned by `exit_slot_step`, derived from the slot's *returned* frame
(`prev_frame`) rather than the ambient `active_frame` — an in-step invoke can swap the ambient
frame, so the returned value is the authoritative source. A within-step frame lifetime `'step`
(`'a: 'step`) threads `classify_dispatch` → `SchedulerView` → `BuiltinFn` → the scheduler's write
primitives, lifting to the run `'a` only at the `lift_kobject` Done boundary.

## Seed-side re-anchor

The MATCH / TRY arm seeds and [`run_user_fn`](../src/machine/core/kfunction/exec.rs)
bind their `it` / parameters — values whose type carries the caller's `'a`, allocated into the
frame arena — inside [`CallArena::with_frame_interior`](../src/machine/core/arena.rs), the
single audited home for that re-anchor. The closure receives the frame's arena re-exposed at a
free `'a` (the C0-irreducible re-exposure: an `'a`-typed value must land in an `'a`-typed arena,
and the frame `Rc` the caller holds heap-pins it) and its child scope re-handed through the
witness-bounded `scope_bounded` brand — so the scope half is *not* fabricated free, only the
arena half is. This is the sole surviving free re-exposure in the protocol.
Arm and body statements then dispatch through the framed scheduler write primitives
(`dispatch_in_active_frame`, `dispatch_body`), which
derive the scope from the active frame and store `Yoked`, so the seed itself persists no
fabricated `&'a`.

## Cross-doc context

The protocol surfaces from five concerns; each owning doc keeps its
topic-specific narrative and cross-links here for the protocol
mechanics:

- [memory-model.md](memory-model.md) — value ownership through
  `RuntimeArena` / `CallArena`, the storage shape, scoping, and
  lifetime erasure that this protocol sits on top of.
- [execution-model.md](execution-model.md) — the dispatch / TCO
  pipeline whose `Tail` rewrite drives `try_reset_for_tail`.
- [typing/functors.md](typing/functors.md) — the per-call type-side
  bind and the deferred return-type dep-finish.
- [typing/modules.md](typing/modules.md) — `USING … SCOPE` allocating
  in the call-site arena so a forwarded bind or window-surfaced
  member outlives the block.
- [error-handling.md](error-handling.md) — TCO frame collapse as
  observed in error traces.

## Verification

- `unanchored_kfuture_no_arena_borrow_does_not_anchor` and
  `unanchored_kfuture_with_arena_borrow_does_anchor` cover both sides
  of the targeted KFuture anchor.
- `fast_lane_closure_escapes_outer_call_and_remains_invocable` and
  `fast_lane_escaped_closure_with_param_returns_body_value` confirm a
  closure returned from its defining frame remains invocable.
- `alloc_object_redirects_self_anchored_value_to_escape_arena` locks
  in the cycle gate: a value carrying an `Rc<FrameStorage>` whose
  `arena()` is the receiving arena allocates into the escape arena
  instead, with the per-call arena's storage left untouched.
- `recursive_tagged_match_no_uaf` runs a user-fn that recurses through
  a `Tagged` parameter via MATCH, exercising the `FrameStorage.outer`
  chain that keeps the call-site arena alive across TCO replace.
- `call_arena_try_reset_for_tail_round_trip` and
  `call_arena_try_reset_for_tail_refuses_when_aliased` pin the
  in-place reset: a unique shell `Rc` resets and re-binds correctly
  against the new outer scope; a second shell `Rc` clone refuses with
  the frame's arena pointer unchanged.
- `call_arena_try_reset_for_tail_allows_reset_under_escaped_storage`
  pins the storage/shell split: an escaped value pinning the
  `FrameStorage` (not the shell) does **not** foreclose reuse — the
  reset installs fresh storage while the escapee's retained Rc keeps
  the pre-reset arena and its allocations alive and aliased.
- `chained_tail_calls_reuse_frames` asserts that a chain of user-fn
  tail calls (`AA → BB → CC → DD → PRINT`) bumps the scheduler's
  tail-reuse counter and collapses to one slot.
- `repeated_user_fn_calls_do_not_grow_run_root_per_call` asserts 50
  ECHO calls grow the run-root arena by exactly 50 — one per-call
  argument value (`Number(7)`) per call, with all per-call scaffolding
  freed at call return. Intermediate node outputs no longer land in
  run-root: a consumed value dies with its consumer's frame, and only a
  consumer-less root drains to the run arena.
- The audit slate runs cycle-free across every unsafe site that
  routes through the protocol under `MIRIFLAGS=-Zmiri-tree-borrows`
  with zero UB and zero process-exit leaks. The canonical slate list
  lives in [observe/miri_slate.md](../observe/miri_slate.md).
