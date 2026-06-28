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
- `KObject::KFunction(&'a KFunction<'a>)` and `KObject::KFuture(KFuture<'a>)`
  hold a bare value-side reference to a function-region slot and reach the
  per-call region that owns the function's captured scope only through that
  reference's scope `region_owner`. They carry no per-value liveness anchor:
  the region an escaping closure / future reaches is pinned by the carrier's
  witness [`FrameSet`](../src/machine/core/arena.rs) while it rides a scheduler
  slot, and retained onto the consumer frame when the value is relocated out
  (see [§ Region lifetime erasure](#region-lifetime-erasure)).
- `Module` and `Signature` cache their declaration scopes as a
  [`BoundedScopePtr`](../src/machine/core/scope_ptr.rs) (heap-pinned by the surrounding
  region chain).

**Directionality rule.** References go inward freely — a per-call region's
slots may point at run-root slots, because the run-root region outlives every
per-call region by the lexical-scoping invariant. A reference that points
*outward* — a value referencing a slot in a dying per-call region, the
canonical case being a closure / module returned from its defining frame —
keeps that region alive through its carrier's witness, never a per-value anchor
on the value itself: a producer slot's `FrameSet` pins it while the value rides
the scheduler, and relocating the value into a consumer's region retains its
reached frame onto the consumer (see
[§ Region lifetime erasure](#region-lifetime-erasure)).

**Why graph rather than tree.** Many-to-one captures and bindings, sibling
scopes sharing an outer, mutual references between a `Scope` and its
region's `scopes` sub-arena, and cross-region carrier-witness pins all
break tree shape. Slots are added incrementally as the program runs;
references can be installed before or after the pointee exists (forward
declarations, replay-park edges). The frame-chain `Rc` that rides on top of
this graph lives in
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

The per-call region's lifecycle — how a relocated value's reached regions are
retained onto the consumer frame (the
[`relocate_carried`](../src/machine/execute/lift.rs) copy plus the carrier-set
retention, with `reached_frame` for the residual type-channel module), how the scheduler propagates the active frame, how
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
lifetimes and erase it once through the safe `SealedExtern::erase` — the per-call frame builds with
no construction-time lifetime fabrication, leaving only the read-side re-attach, which takes the
pinning `Rc` as an explicit witness so it carries no in-situ `unsafe` either.

Two further carriers hold *no* lifetime and so cannot brand `'a`: `CallFrame`'s per-call child scope
(non-generic — it backs `Rc<CallFrame>`) and a scheduler slot's `NodeScope::YokedChild` (a
cart-ancestor block scope evicted off the lifetime-free node). The frame's child scope rides the
substrate's externally-witnessed [`SealedExtern<ScopeRefFamily>`](../src/witnessed.rs) carrier; the
`YokedChild` carrier stays an [`ErasedScopePtr`](../src/machine/core/scope_ptr.rs) (a cross-node
erasure outside the per-call struct). Both store a `&'static Scope` erased once on the store side
through the safe `erase_to_static::<ScopeRefFamily>` (forgetting a reference's lifetime for storage
cannot fabricate one), so the handle holds the reference outright and carries **no `unsafe`** — there
is no `NonNull` deref. Each recovers the content lifetime on read through a **fully safe**
witness-bounded accessor — `SealedExtern::attach` for the frame, `ErasedScopePtr::reattach_witnessed`
for `YokedChild`, both of signature `<'w, 'b: 'w, W: Witness>(&'w self, &'w W) -> &'w Scope<'b>` and
both the scope-pointer analog of [`reattach_with`](../src/witnessed.rs). The witness is **external**
(the frame `Rc`, which for a `YokedChild` pins the ancestor region through its `FrameStorage.outer`
chain) and not expressible in the *carrier's* type, so rather than fabricate the lifetime in-situ the
re-attach takes it as an explicit `Witness` borrow: `'w` is bounded by that borrow, content `'b` is
free, and the only `unsafe` it routes is the shared `retype` inside the witnessed `reattach_ref_with`
(in `witnessed.rs`). The `CallFrame` accessors (`scope` / `scope_for_bind` / `scope_bounded`) are
thin **safe** wrappers that pass the frame's own storage `Rc` as the witness, so every frame-scope
fabrication funnels through the one `attach` and call sites carry no `unsafe`.

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
transient `reattach_value` / `reattach_ref` helpers, the witness-borrowed
`reattach_with` / `reattach_ref_with`, the `Witnessed` accessors, and the region's
store-side `erase_to_static` all route it. The carrier families live beside their own
types as declarative `unsafe impl Reattachable` instantiations — `ContractFamily` for the
node's [`ErasedContract`](../src/machine/core/kfunction/body.rs), `CarriedFamily` /
`ContinuationFamily` for the scheduler value (`Workload::Value`) and continuation
(`Workload::Continuation`), `RegionRefFamily` for the consumer region the run-loop step opens its
tail against, and `ScopeFamily` so the scope-pointer handles re-attach and the
region's `&Scope → &Scope<'static>` storage erasures route the same primitive — so `witnessed.rs`
names no concrete Koan type and the scheduler stays workload-independent (the workload depends on
the substrate for the machinery, not the reverse).

[`Witnessed<T, W>`](../src/witnessed.rs) bundles an erased carrier `Erased<T>` with the liveness
witness `W` that pins its pointee in one value, so "the witness keeps the value alive" is a type
invariant rather than a co-stored field pair plus a SAFETY comment. `W` is a [`Witness`](../src/witnessed.rs)
— an `unsafe` marker asserting its pointee stays at a fixed address while held; `Rc<F>` qualifies
(a static `StableDeref` assert records the obligation), and a *set* of them — the Koan result-slot
and lift witness [`FrameSet`](../src/machine/core/arena.rs) — pins every region a value reaches at
once, an empty set being a frameless / run-region terminal whose backing outlives the carrier. The carrier is re-anchored through one
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
re-seals under the *combined* witness — the union of both operands' regions, with `outer`-chain
subsumption dropping a region another already pins (the `MergeWitness` trait's `merge`), returning
`None` only when a single-region witness cannot represent two unrelated regions. All keep their `unsafe` retype inside the module, so callers carry none; `yoke`
in fact routes only the safe `erase`, carrying no retype of its own.

The value channel is borrow-checked end to end. The scheduler stores a finalized terminal as a single
`Sealed<W::Value, W::Witness>` ([`node_store.rs`](../src/scheduler/node_store.rs)) — the
opaque dormant form of a `Witnessed` carrier, which hides every transform (`with` / `map` / `yoke` /
`merge`) and re-anchors only through the rank-2 destination verb `Sealed::open` or the transitional
borrow-bounded `Sealed::read`. `finalize` bundles the erased value with its producer frame's witness
(a singleton `FrameSet`) and seals it (an empty set is a frameless / run-region terminal). A read (`read_result` / `read` /
`read_result_with_frame`) goes through `Sealed::read` — which delegates to `Witnessed::read` —
re-anchoring to the read's own `&self` borrow — `Live<'node, W>`. Because
`free_one` / `finalize` need `&mut self`, the bundled witness cannot drop while a read borrow is
live, so the re-anchored `'node` lifetime cannot outlive the backing region: the pin-outlives-read
fact is a borrow the compiler checks. The driver's transient reads
([`KoanRuntime::read_result`](../src/machine/execute/runtime.rs), the
[`SchedulerView`](../src/machine/execute/dispatch/ctx.rs) forwarder) consume that `'node` value with
no `unsafe` of their own. The continuation and contract carriers — stored `Erased` on the
lifetime-free node — re-anchor through the run-loop step's **consuming, externally-witnessed**
`Sealed::open`: [`run_step`](../src/machine/execute/run_loop.rs) zips the continuation, the contract,
and the consumer region and opens them at one rank-2 `for<'b>` brand standing in for the step
lifetime, witnessed by the held start cart `Rc` (whose `outer` chain subsumes the contract's home),
so the whole tail nests inside the brand and carries no loose witness-borrow reattach. The
consumer-pull lift and the `Outcome::Forward` ready pull re-anchor their reads at a *node* lifetime,
not a fabricated `'run`: each dep terminal is read out borrow-bounded, erased into one
`DepResultsFamily` slice carrier, and opened **in-band** at `'b` alongside the continuation. Inside
that brand [`relocate_carried`](../src/machine/execute/lift.rs) copies each dep into the consumer
`dest` region with a plain `'b → 'b` structural alloc — the composite spine sharing its `Rc` payloads,
a closure / future / module riding its bare `&'b` borrow into the source region — and the
`Outcome::Forward` pull lands in that same region at the brand, so every dep value is born at `'b`
with no reattach of its own beyond the one step `open`. There is **no value-path `unsafe`** left: the
relocation allocs at the destination region's own lifetime, so the lift hook is a safe
`deep_clone` + `alloc`. The relocation seam `Sealed::transfer_into` wraps this as a `merge` — the
relocated value re-sealed under the set union of every region it still reaches (its retained sources ∪
`dest`) — and the storage-bound drain / forward path routes it via
[`relocate_terminal`](../src/machine/execute/runtime.rs). The consumer-less root drain in
[`run_program`](../src/machine/execute/runtime/interpret.rs) relocates each top-level terminal into the
run-global root region the same way.

A relocated closure / future / module survives its producer's dying frame because the copy keeps its
bare borrow and the *consumer* frame keeps that borrow's region alive. A **closure / future** carries
the regions it reaches on its [delivered carrier](per-node-memory.md#storage-and-access-seal-open-transfer_into):
the embedding or binding site folds that carrier (`merge` at an `attr` / `FROM` projection, `fold_reach`
at a `let` / user-fn arg bind), and the root drain `retain`s the rehomed terminal's full witness set
onto `FrameStorage.retained` — so a multi-region value keeps *every* region it reaches. The one value
still reconstructed is the type channel's not-yet-witnessed `KType::Module`: `reached_frame`
([`lift.rs`](../src/machine/execute/lift.rs)) recovers its child scope's defining frame from the value's
`region_owner`, and the consumer frame `retain`s that single frame into `FrameStorage.retained` at the
read-out boundaries — the `run_step` relocate, the root drain, and the `extract_terminal` test harness —
until [`alloc_ktype`](../roadmap/per-node-memory/alloc-ktype-witnessed.md) takes it off and deletes the
reconstruction. No cycle forms: a dispatched frame's `outer` is `None`, so a retained descendant never
strong-refs back, and `retain` drops a frame whose region an ancestor already pins.

The per-call frame's seed binds (MATCH / TRY `it`, `KFunction::invoke` params) reach the per-call
region through the child scope's own `region` field — a `Copy` `&'a KoanRegion` reached via
[`CallFrame::with_frame_interior`](../src/machine/core/arena.rs), pinned by the held frame `Rc` — so
they fabricate no reference of their own. The store side carries no `unsafe` at all: a lifetime-free
handle's `erase` forgets the scope reference's lifetime through the safe `erase_to_static`, and the
branded `BoundedScopePtr::erase` casts a live reference, both deferring every fabrication hazard to
the re-attach.

The allocation engine needs **no cycle gate**: a stored value holds no owning `Rc` back to a region —
a closure / future / module is a bare borrow into its defining region, kept alive by its carrier's
witness set rather than an embedded anchor — so storing it where requested can never close an
allocation back-edge. Every family implements the `Stored` trait and routes the one
[`alloc`](../src/witnessed/region.rs) engine, which erases the value to `'static`, stores it in the
family's sub-arena, and re-anchors the store to `'a`; the engine carries no redirect logic. It stays
unbypassable by construction: the substrate's `storage` bundle is private and `alloc` is the only path
to it, so no `Stored` impl can route around the engine.

A [`CallFrame`](../src/machine/core/arena.rs) is a thin shell over a refcounted
[`FrameStorage`](../src/machine/core/arena.rs): the shell carries a `Rc<FrameStorage>` and an
`Option<SealedExtern<ScopeRefFamily>>` (the child scope; `None` only transiently during construction), while
`FrameStorage` bundles the `KoanRegion` and an `Option<Rc<FrameStorage>>` for the parent-frame
chain. The shell/storage split lets an escaping value pin only the storage, leaving the shell
uniquely owned for tail reuse (see
[per-call-region/frames.md § TCO frame reuse](per-call-region/frames.md#tco-frame-reuse)). Two
invariants make the ownership unit coherent:

- **Heap-pinning via `Rc`.** `CallFrame::new` builds the region inside its own
  `Rc<FrameStorage>` and only ever exposes the frame as `Rc<CallFrame>`, so the inner
  region's heap address is stable for the storage Rc's life and `scope_carrier` (a
  `&'static Scope` into `region.scopes`) stays valid alongside it. Accessors re-attach lifetimes
  anchored to `&self`. A tail reset installs a *fresh* `FrameStorage`, so the region
  address changes across a reset — no accessor captures it across one, and the borrow
  checker forbids safe code from doing so.
- **Field declaration order encodes drop order.** On `FrameStorage`, `region` is declared
  before `outer` so the auto-derived `Drop` tears down this frame's region *before*
  releasing the parent storage Rc; on the shell, `storage` is declared before `scope_carrier`.
  Inner references die before the outer storage they may reference, ruling out a dangling
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
- Per-call-region protocol verification (escaping-value relocation and retention, TCO
  frame reuse, MATCH `FrameStorage.outer` chain) is enumerated in
  [per-call-region/scope-handles.md § Verification](per-call-region/scope-handles.md#verification).
- The audit slate runs cycle-free across every unsafe site in the runtime
  under `MIRIFLAGS=-Zmiri-tree-borrows` with zero UB and zero process-exit
  leaks, signing off the memory model as it stands today. The canonical
  slate list lives in [observe/miri_slate.md](../observe/miri_slate.md).

## Open work

The remaining per-node-memory migrations — wiring `alloc_ktype` to return a co-located `Witnessed`
carrier (the object channel is already witnessed end-to-end), and moving the residual witness-borrow
read paths onto the `Sealed` access verbs — are tracked by the
[per-node-memory roadmap project](../roadmap/per-node-memory/). See
[per-node-memory.md § Open work](per-node-memory.md#open-work) for the dependency ordering.
