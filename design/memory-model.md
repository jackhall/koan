# Memory model and scoping rules

Every `KObject` lives in a [`KoanRegion`](../src/machine/core/arena.rs). Top-level
work allocates into the **run-root region**; each user-fn call gets its own
**per-call `KoanRegion`** owned by [`CallFrame`](../src/machine/core/arena.rs),
freed when the call's slot finalizes.

## Storage shape: a graph of region slots

A `KoanRegion` holds seven `typed_arena`-backed sub-arenas — for `KObject`,
`KFunction`, `Scope`, `Module`, `Signature`, `KType`, and `OperatorGroup`. Slots have stable
heap addresses; the runtime carries cross-references between them rather
than ownership trees. The structural edges:

- `Scope.outer: Option<BoundedScopePtr<'a>>` — the lexical-parent chain, a
  content-branded handle. Many sibling scopes can share one outer, so the
  in-degree is unbounded.
- `Scope.region: &'a KoanRegion` — back-pointer to the owning region.
- [`Bindings.data`](../src/machine/core/bindings.rs) maps each bound name
  to a `&'a KObject<'a>`. The pointee may live in this scope's region or in
  an outer one.
- [`KFunction.captured`](../src/machine/core/kfunction.rs) holds a
  [`BoundedScopePtr`](../src/machine/core/scope_ptr.rs) — the closure's definition
  scope, lifetime-erased. Multiple `KFunction`s share one captured scope when
  they were defined in the same body.
- `KObject::KFunction(&'a KFunction<'a>, Option<Rc<FrameStorage>>)` and
  `KObject::KFuture(KFuture, Option<Rc<FrameStorage>>)` carry both a value-side
  reference to a function-region slot and an optional `Rc<FrameStorage>` anchor
  to the per-call region that owns the function's captured scope.
- `Module` and `Signature` cache their declaration scopes as a
  [`BoundedScopePtr`](../src/machine/core/scope_ptr.rs) (heap-pinned by the surrounding
  region chain).

**Directionality rule.** References go inward freely — a per-call region's
slots may point at run-root slots, because the run-root region outlives every
per-call region by the lexical-scoping invariant. References that need to
point *outward* — a lifted value referencing a slot in a dying per-call
region — must carry an `Rc<FrameStorage>` anchor on the value (or its enclosing
variant) so the per-call region survives. The lift machinery enforces this at
the region boundary; see
[per-call-region/lifecycle.md § Lift-time anchor decision](per-call-region/lifecycle.md#lift-time-anchor-decision).

**Why graph rather than tree.** Many-to-one captures and bindings, sibling
scopes sharing an outer, mutual references between a `Scope` and its
region's `scopes` sub-arena, and cross-region `Rc<FrameStorage>` anchors all
break tree shape. Slots are added incrementally as the program runs;
references can be installed before or after the pointee exists (forward
declarations, replay-park edges). The cycle gate and the frame-chain `Rc`
that ride on top of this graph live in
[per-call-region/README.md](per-call-region/README.md).

The graph shape is also why the runtime stores `*const T<'static>` and
transmutes on access: a self-referential graph of incrementally added
slots with cross-references doesn't fit the one-owner-builds-one-dependent
shape that self-referential-struct crates model.

## Scoping: lexical

Free names in a user-fn body resolve through the function's **definition**
scope, carried on [`KFunction.captured`](../src/machine/core/kfunction.rs) — not the
call-site scope. Top-level `FN` definitions capture the run-root, so their free
names resolve through it; nested `FN`s correctly close over their enclosing
locals.

Lexical scoping is what makes the F_{k+1}→F_k chain in tail-recursive code O(1)
memory. Without it, a recursive call would resolve the recursive name through
the call-site scope and pin every prior frame's bindings alive.

## Per-call region protocol

The per-call region's lifecycle — which `KObject` variants carry an
`Option<Rc<FrameStorage>>` anchor, how
[`lift_kobject`](../src/machine/execute/lift.rs) decides to attach
one, how the `alloc_object` cycle gate routes self-referential
allocations, how the scheduler propagates the active frame, how
builtin-built frames chain the call-site frame's storage through
`FrameStorage.outer`, and how the TCO step reuses the frame shell over a
fresh `FrameStorage` — is documented in
[per-call-region/README.md](per-call-region/README.md). This file
keeps the storage-shape, scoping, and lifetime-erasure scaffolding the
protocol sits on top of.

## Region lifetime erasure

Every sub-arena inside [`KoanRegion`](../src/machine/core/arena.rs) stores
`T<'static>` rather than `T<'a>` — the `'static` is phantom so `KoanRegion`
itself carries no lifetime parameter. The erase-store engine lives generically in
the [`Region<W>`](../src/witnessed/region.rs) substrate (`KoanRegion`
is the Koan instantiation `Region<KoanStorageProfile>`). Each named `alloc*` wrapper
takes input at the caller's `'a` and routes one `alloc<K: Stored>` engine: the engine
erases the value into its `'static` lifetime family (`At<'static>`) for storage and
re-anchors the returned `&'a` to the input borrow on the way out. The store-side erasure
routes the scheduler's single audited `erase_to_static` — the safe direction of the one
`retype` primitive (described below) — so the region's store-side and the scheduler's
read-side share one transmute rather than each carrying its own. Each `Stored` family is a
`Reattachable` family (`At<'static> == Self`), the GAT both directions key on. It is sound
because:

- Lifetimes are zero-sized, so `T<'a>` and `T<'static>` have identical layout.
- `alloc*` returns an `&'a` tied to the input borrow; no `'static` reference
  ever escapes.
- On drop, no stored value's `Drop` impl follows a lifetime-parameterized
  reference — auto-derived `Drop` only touches owned contents. Sub-regions
  drop together at `KoanRegion` drop, so any cross-sub-arena `&` is dead
  by the time anyone could observe it.

The scope-pointer case — `CallFrame`, `Module`, `Signature`, `KFunction`, and a `Scope`'s
own lexical parent each holding a pointer to a captured, defining, or parent `Scope` — is
centralized in two branded handles in
[`scope_ptr.rs`](../src/machine/core/scope_ptr.rs), split on **whether the carrier can brand the
scope's `'a`**.

The **safe** [`BoundedScopePtr<'a>`](../src/machine/core/scope_ptr.rs) backs every carrier that owns
a real `'a`: `KFunction::captured`, `Module::child_scope`, `Signature::decl_scope`, and a `Scope`'s
`outer` lexical parent and `root` handle. `erase(&Scope<'a>)` records the content `'a` in a
`PhantomData<&'a Scope<'a>>` brand (which, because `Scope<'a>` is invariant, also pins the carrier
invariant in `'a`); `get(&'p self) -> &'p Scope<'a>` re-hands the content `'a` behind a
reader-bounded borrow. Because the free content `'a` is never cashed *unbounded*, a shorter witness
borrow cannot fabricate a longer-lived reference, so `get` needs no borrow==content coupling and
carries **no `unsafe`**.

`BoundedScopePtr` also carries two **brand-shortening** helpers — `erase_shortened<'long: 'a>(&Scope<'long>)`
and `shortened<'short>(self) where 'a: 'short` — that brand a longer-lived scope at a shorter `'a`.
Narrowing the brand *under-claims* the pointee's real life (`get` only ever re-hands at the branded
`'a`), so both are safe by construction (pointer cast + phantom, no `unsafe`). They let
[`Scope::child_for_frame`](../src/machine/core/scope.rs) build a per-call child against its **fresh**
per-call region's lifetime while brand-shortening the longer-lived lexical parent and run-global root
to that lifetime, so the child needs no common lifetime with its parent. That is what lets
`CallFrame::new` / `try_reset_for_tail` construct the per-call child at real (non-`'static`)
lifetimes and erase it once through the safe `ErasedScopePtr::erase` — the per-call frame builds with
no construction-time lifetime fabrication, leaving only the read-side re-attach, which takes the
pinning `Rc` as an explicit witness so it carries no in-situ `unsafe` either.

The remaining [`ErasedScopePtr`](../src/machine/core/scope_ptr.rs) backs the two carriers that hold
*no* lifetime and so cannot brand `'a`: `CallFrame`'s per-call child scope (non-generic — it backs
`Rc<CallFrame>`) and a scheduler slot's `NodeScope::YokedChild` (a cart-ancestor block scope evicted
off the lifetime-free node). Both store through the safe `erase(&Scope<'_>)` (forgetting a lifetime
for storage cannot fabricate one) and recover the content lifetime on read through the
**safe-signature** `reattach_witnessed<'w, 'b: 'w, W: Witness>(&'w self, &'w W) -> &'w Scope<'b>` —
the scope-pointer analog of [`reattach_with`](../src/witnessed.rs). The witness is **external** (the
frame `Rc`, which for a `YokedChild` pins the ancestor region through its `FrameStorage.outer` chain)
and not expressible in the *carrier's* type, so rather than fabricate the lifetime in-situ the
re-attach takes it as an explicit `Witness` borrow: `'w` is bounded by that borrow, content `'b` is
free, and the lone `unsafe` is the `NonNull` deref inside the one method (the content retype routes
the witnessed `reattach_ref_with`). The `CallFrame` accessors (`scope` / `scope_for_bind` /
`scope_bounded`) are thin **safe** wrappers that pass the frame's own storage `Rc` as the witness, so
every frame-scope fabrication funnels through the one `reattach_witnessed` and call sites carry no
`unsafe`.

Beyond the store-side erasure and the branded scope pointers, a handful of carriers store a
borrow-carrying *value* on a structure the borrow checker cannot lifetime-track — a scheduler
node's slot, a per-call `TraceFrame` — and re-anchor it at a caller-chosen lifetime on read,
witnessed by a held `Rc`. The erase/reattach discipline that makes the move safe lives in the
top-level [`witnessed`](../src/witnessed.rs) module, a sibling of `machine` and `scheduler` that
names no concrete workload type: both depend on it for the machinery, not the reverse.
[`witnessed.rs`](../src/witnessed.rs) declares `unsafe trait Reattachable { type At<'r>; }` —
a family whose representation is identical across every choice of its single lifetime — and
[`Erased<T>`](../src/witnessed.rs) stores that family's `At<'static>` form. A single
private `retype<A, B>` — a `transmute_copy` through a `ManuallyDrop` (plain `transmute` cannot prove
two opaque GAT projections share a size), guarded by a `const` size assert that restores the check
`transmute` would emit — is the only place a
`T::At<'a> → T::At<'b>` lifetime retype is written; `Erased::erase` / `Erased::reattach`, the
transient `reattach_value` / `reattach_ref` / `reattach_slice_with` helpers, the witness-borrowed
`reattach_with` / `reattach_ref_with` / `vend_carrier`, the `Witnessed` accessors, and the region's
store-side `erase_to_static` all route it. The carrier families live beside their own
types as declarative `unsafe impl Reattachable` instantiations — `ContractFamily` for the
node's [`ErasedContract`](../src/machine/core/kfunction/body.rs), `CarriedFamily` /
`ContinuationFamily` for the scheduler value (`Workload::Value`) and continuation
(`Workload::Continuation`), `ResultCarriedFamily` for the transient step-lifetime re-anchor
(`deps_at_step`) in `outcome.rs`, and `ScopeFamily` so the scope-pointer handles re-attach and the
region's `&Scope → &Scope<'static>` storage erasures route the same primitive — so `witnessed.rs`
names no concrete Koan type and the scheduler stays workload-independent (the workload depends on
the substrate for the machinery, not the reverse).

[`Witnessed<T, W>`](../src/witnessed.rs) bundles an erased carrier `Erased<T>` with the liveness
witness `W` that pins its pointee in one value, so "the witness keeps the value alive" is a type
invariant rather than a co-stored field pair plus a SAFETY comment. `W` is a [`Witness`](../src/witnessed.rs)
— an `unsafe` marker asserting its pointee stays at a fixed address while held; `Rc<F>` qualifies
(a static `StableDeref` assert records the obligation) and `Option<W>` lifts it for a frameless
terminal whose backing region outlives the carrier (`None`). The carrier is re-anchored through one
of three read/transform accessors, all sound by construction: `with` re-anchors behind a **rank-2**
`for<'b>` brand so the fabricated content lifetime cannot escape the closure into the result (the
generativity trick; the naive content-free reattach is a Miri-proven use-after-free); `map` consumes
and re-projects under the same brand and witness (`yoke::map_project`'s shape); and `read` hands the
carrier out bounded by the `&self` borrow itself, sound because the content lifetime *is* the borrow
the bundled witness pins, not a free `'b` the caller could widen. Two build-time accessors close the
co-location gap `new` leaves to caller assertion: `yoke` *sources* a carrier from the witness's own
region behind a `for<'b>` brand (over the `WitnessRegion` trait), so the only references the carrier
can hold are region-derived — the witness-pins-the-value invariant holds by construction rather than
asserted; and `merge` combines two carriers under one shared brand, runs a binding projection, and
re-seals under the *descendant* witness (the one whose `outer` ancestor chain transitively pins both
regions, selected via the `MergeWitness` trait's `merge_pin`), rejecting unrelated witnesses before
the projection runs. All keep their `unsafe` retype inside the module, so callers carry none; `yoke`
in fact routes only the safe `erase`, carrying no retype of its own.

The value channel is borrow-checked end to end. The scheduler stores a finalized terminal as a single
`Sealed<W::Value, Option<Rc<W::Cart>>>` ([`node_store.rs`](../src/scheduler/node_store.rs)) — the
opaque dormant form of a `Witnessed` carrier, which hides every transform (`with` / `map` / `yoke` /
`merge`) and re-anchors only through the rank-2 destination verb `Sealed::open` or the transitional
borrow-bounded `Sealed::read`. `finalize` bundles the erased value with its producer frame `Rc` and
seals it (the `None` arm is a frameless / run-region terminal). A read (`read_result` / `read` /
`read_result_with_frame`) goes through `Sealed::read` — which delegates to `Witnessed::read` —
re-anchoring to the read's own `&self` borrow — `Live<'node, W>`. Because
`free_one` / `finalize` need `&mut self`, the bundled frame `Rc` cannot drop while a read borrow is
live, so the re-anchored `'node` lifetime cannot outlive the backing region: the pin-outlives-read
fact is a borrow the compiler checks. The driver's transient reads
([`KoanRuntime::read_result`](../src/machine/execute/runtime.rs), the
[`SchedulerView`](../src/machine/execute/dispatch/ctx.rs) forwarder) consume that `'node` value with
no `unsafe` of their own. The continuation and contract carriers — stored `Erased` on the
lifetime-free node — re-anchor through [`vend_carrier`](../src/witnessed.rs), whose returned `'w` the
compiler bounds against a witness borrow `&'w Rc<W::Frame>` the driver passes (the slot's cart for
the continuation in `run_step`, the producer frame for the contract at the Done boundary), so those
call sites carry no `unsafe` either. The consumer-pull lift and the `Outcome::Forward` ready pull
re-anchor their reads at a *node* lifetime, not a fabricated `'run`: `read_lifted` forwards a
frameless terminal through the witness-borrowed `reattach_with` and copies a framed terminal into the
consumer's region through [`lift`](../src/machine/execute/lift.rs), and the node→step re-anchors
(`deps_at_step`, the `Outcome::Forward` shorten) are safe `reattach_slice_with` / `reattach_with`
bounded by the step-held cart `Rc`. The consumer-less root drain in
[`run_program`](../src/machine/execute/runtime/interpret.rs) lifts each top-level terminal into the
run-global root region directly through `lift`. The single irreducible audited `unsafe` reattach in
the value path is `lift`'s own value-relocation re-anchor: a value about to be copied out has no
*borrowed* witness to bound the target lifetime, but `src` heap-pins the value's region for the copy
and `lift_kobject` self-anchors any surviving borrow into the destination via an embedded `Rc` — the
same self-anchoring shape as `Erased::reattach`.

A sibling primitive in [`reattach.rs`](../src/machine/core/reattach.rs), `pin_deref`, owns the
*other* unsafe shape — re-borrowing a raw `*const T` whose pointee a heap pin holds fixed. Its one
caller is [`CallFrame::with_frame_interior`](../src/machine/core/arena.rs): the held frame `Rc`
heap-pins the per-call region, which is re-exposed at a free `'a` for the seed binds (MATCH / TRY
`it`, `KFunction::invoke` params). The storage engine's cycle-gate escape redirect needs no
`pin_deref`: `Region` holds its escape target as an owning `StorageProfile::EscapeOwner` (the Koan
`FrameRegionPin`, an `Rc<FrameStorage>` deref'd to its region), so the redirect is a borrow the
checker proves. Erase/reattach moves a value between lifetimes; `pin_deref` recovers a reference from
a pointer the borrow checker never tracked, so it stays in `machine::core` as the one audited home
for that single frame-interior `&*ptr`. The
store side carries no `unsafe` at all: each handle's `erase` builds its stored pointer with the safe
`NonNull::from(scope).cast()`, deferring every fabrication hazard to the re-attach.

Every family implements the `Stored` trait and routes the one gated
[`alloc`](../src/witnessed/region.rs) engine. `anchors_to` is a required trait
method, so each family declares its cycle behavior at its impl site: `KObject` and
`KType` walk their composite tree for a self-targeting `Rc<FrameStorage>`, while the
families that cannot hold one — `KFunction`, `Scope`, `Module`, `Signature`, and
`OperatorGroup` — declare `anchors_to => false`. The gate is therefore uniform and
unbypassable by construction: `Stored` is unsealed (an in-crate extension point), but
the substrate's `storage` bundle is private and `alloc` is the only path to it, so no
impl can route a value around the redirect. A self-anchoring value redirects to the
escape region no matter which wrapper stored it.

A [`CallFrame`](../src/machine/core/arena.rs) is a thin shell over a refcounted
[`FrameStorage`](../src/machine/core/arena.rs): the shell carries a `Rc<FrameStorage>` and an
`Option<ErasedScopePtr>` (the child scope; `None` only transiently during construction), while
`FrameStorage` bundles the `KoanRegion` and an `Option<Rc<FrameStorage>>` for the parent-frame
chain. The shell/storage split lets an escaping value pin only the storage, leaving the shell
uniquely owned for tail reuse (see
[per-call-region/frames.md § TCO frame reuse](per-call-region/frames.md#tco-frame-reuse)). Two
invariants make the ownership unit coherent:

- **Heap-pinning via `Rc`.** `CallFrame::new` builds the region inside its own
  `Rc<FrameStorage>` and only ever exposes the frame as `Rc<CallFrame>`, so the inner
  region's heap address is stable for the storage Rc's life and `scope_ptr` (a raw
  pointer into `region.scopes`) stays valid alongside it. Accessors re-attach lifetimes
  anchored to `&self`. A tail reset installs a *fresh* `FrameStorage`, so the region
  address changes across a reset — no accessor captures it across one, and the borrow
  checker forbids safe code from doing so.
- **Field declaration order encodes drop order.** On `FrameStorage`, `region` is declared
  before `outer` so the auto-derived `Drop` tears down this frame's region *before*
  releasing the parent storage Rc; on the shell, `storage` is declared before `scope_ptr`.
  Inner pointers die before the outer storage they may reference, ruling out a dangling
  `outer` during drop.

A scheduler slot's scope handle is lifetime-free, so the node carries no `'run` through its scope.
A per-call frame scope is stored as a payload-less
[`NodeScope::Yoked`](../src/machine/execute/nodes.rs) marker re-projected from the slot's own
`Node.frame` cart; a genuinely run-lived scope (a binder body's decl-scope child) is stored
as `NodeScope::YokedChild`, an erased `ErasedScopePtr` re-attached at read through the safe-signature
`reattach_witnessed`, witnessed by the slot's cart `Rc`.
Both arms ride a grouped `NodePayload` (scope handle + lexical chain) alongside the slot's frame. The
slot-storage scope handle and the seed-side `with_frame_interior` re-anchor are documented in
[per-call-region/scope-handles.md § Slot-table scope handle](per-call-region/scope-handles.md#slot-table-scope-handle).

## Re-entrant scope writes

[`Scope::bind_value`](../src/machine/core/scope.rs),
[`Scope::register_function`](../src/machine/core/scope.rs), and
[`Scope::register_type`](../src/machine/core/scope.rs) route through
the embedded [`Bindings`](../src/machine/core/bindings.rs) façade's
validated write primitives (`try_apply` / `try_register_function` /
`try_register_type`), which `try_borrow_mut` the relevant map
(`data` / `functions` / `types`) and return
`ApplyOutcome::Conflict` when a borrow is already held. The scope then defers
the write through the embedded
[`PendingQueue`](../src/machine/core/pending.rs) façade
(`defer_value` / `defer_function` / `defer_type`); the queue is drained by
[`Scope::drain_pending`](../src/machine/core/scope.rs), invoked by the
scheduler between dispatch nodes, which calls `PendingQueue::drain(&Bindings)`
to replay each deferred write through the same validated `Bindings` write path
as a direct insert. The hot path (no concurrent borrow) is one direct insert
with the function-mirror write folded in. Re-entrant writes queue silently and
become visible after the iterating borrow releases, with snapshot-iteration
semantics for the iterator. Drain-time `Err` returns trip a `debug_assert!`
in debug builds (by drain time these are invariant violations); release
builds keep the legacy silent-drop behavior so dispatch nodes never see
surfaced errors.

## Structural invariants

Several "must hold" rules are encoded in types rather than checked at runtime:

- `Scope::region: &'a KoanRegion` is non-optional; `test_sink()` takes a
  caller-supplied region.
- `KFunction::captured_scope() -> &'a Scope<'a>` is non-optional.
- The running scope passes through `KoanRuntime::dispatch_in_scope(expr, scope)`
  directly, so dispatch sites carry their scope explicitly.
- [`KoanRegion::alloc_function`](../src/machine/core/arena.rs) `debug_assert`s
  region-identity between the function and its captured scope, catching a
  misallocated KFunction at the allocation site rather than later as a
  use-after-free in `lift_kobject`'s fast path.

## Performance notes

The push/notify scheduler ([execution/README.md § Push/notify dependency
edges](execution/scheduler.md#pushnotify-dependency-edges)) keeps its slot-table
state in a
[`NodeStore`](../src/scheduler/node_store.rs)
sub-struct that owns `slots: SlotVec<SlotState<'run>>` (each slot a `PreRun(Node)`
/ `Running` / `Done(Result<Carried, KError>)` / `Aliased(NodeId)` / `Free`) and
`free_list: Vec<NodeId>`, behind the slot lifecycle
`alloc_slot → take_for_run → reinstall* → finalize → free_one`. `alloc_slot` is
the only path that picks an index (pulling from `free_list` before extending
`slots`), `finalize` is the only path that lands a terminal `Done`, and
`free_one` is the only path that returns a slot to `Free` and pushes its index
onto `free_list`. Dependency bookkeeping lives alongside it in a
[`DepGraph`](../src/scheduler/dep_graph.rs) sub-struct
that bundles three `Vec`-shaped fields: `notify_list: Vec<Vec<NodeId>>`
(each producer's dependent list), `pending_deps: Vec<usize>` (each consumer's
unresolved-dep counter), and `dep_edges: Vec<Vec<DepEdge>>` (each slot's
backward edges to producers, tagged `DepEdge::Owned(NodeId)` for sub-slots
the consumer is responsible for reclaiming and `DepEdge::Notify(NodeId)` for
sibling producers the consumer only parked on for wake notification). All
three are 1:1 with `NodeStore`'s slot count; the fields are private and
mutated only through `DepGraph`'s atomic-update methods, so the tri-vector
invariant (every forward edge in `notify_list[p]` matched by a backward
`dep_edges[c]` entry and a +1 in `pending_deps[c]`) is enforced by the
surface rather than by convention.

Transient-node reclamation runs through `Scheduler::reclaim_deps` from the
unified node handler `KoanRuntime::run_step`, *after* the finish closure returns
its `Outcome` but *before* the harness applies it. So
when a dispatch splice finish has rewritten `working_expr.parts` to
`ExpressionPart::Future`, the freed indices are back on the free-list before
the harness dispatches the bound expression — its `add()` can recycle them
immediately. `reclaim_deps` clears `dep_edges[idx]` and
invokes `Scheduler::free` per dep index; the walk follows `DepGraph::owned_children`,
which only yields `DepEdge::Owned` arms (`Notify` arms are filtered
inside `DepGraph`), so reclaiming a consumer cannot reach a sibling
producer's subtree through a park edge. It skips any still-live slot
via the `NodeStore::is_live` guard, so a free that dives into another
in-flight user-fn call leaves that subtree for that call's own reclamation.

## Verification

- [`add_during_active_data_borrow_queues_and_drains`](../src/machine/core/scope.rs)
  holds a `data` borrow, calls `bind_value`, drops the borrow, drains, and
  confirms the queued write applied — exercising the conditional-defer path.
- Per-call-region protocol verification (lift anchors, cycle gate, TCO
  frame reuse, MATCH `FrameStorage.outer` chain) is enumerated in
  [per-call-region/scope-handles.md § Verification](per-call-region/scope-handles.md#verification).
- The audit slate runs cycle-free across every unsafe site in the runtime
  under `MIRIFLAGS=-Zmiri-tree-borrows` with zero UB and zero process-exit
  leaks, signing off the memory model as it stands today. The canonical
  slate list lives in [observe/miri_slate.md](../observe/miri_slate.md).

## Open work

- *(none currently tracked)*
