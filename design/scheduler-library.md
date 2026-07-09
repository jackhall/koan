# The scheduler library

Koan's runtime substrate — the deferred-work scheduler, the region memory
system, and the witnessed carrier machinery — is a self-contained library
stack with no dependency on Koan's language semantics. It ships as two
workspace crates: `workcell` *(working name — [workcell.md](workcell.md))*,
the computation-cell substrate (witnessed memory plus a cell table:
continuations, memory anchors, inter-cell values — no acyclicity, no
terminality), and `workgraph`, the DAG scheduler layered on it (dep edges,
park/notify, cycle detection, terminal results, retention, splicing). The
dependency direction (`koan` → `workgraph` → `workcell`, never the reverse)
is what makes "no Koan type in scope" compile-enforced rather than a
convention. Koan is its first embedder, re-exporting `workgraph::witnessed`
and `workgraph::scheduler` from its own crate root so internal
`crate::witnessed::…` / `crate::scheduler::…` paths keep resolving unchanged;
the library is extractable for other embedders. Its public surface is
memory-safe **by construction**: an embedder can schedule work, allocate
values, and pass borrow-carrying results between nodes without writing
`unsafe` and without upholding any convention the compiler cannot check.
Every memory-safety invariant is either enforced by a type (a brand, an
opaque set, a sealed carrier) or discharged inside the library.

This doc owns the *division of responsibility*: what is library, what is
Koan, and the API surface between them.
[per-node-memory.md](per-node-memory.md) owns the witnessed substrate
mechanics; [execution/](execution/README.md) owns the pipeline;
[memory-model.md](memory-model.md) owns Koan's value-ownership semantics;
[per-call-region/](per-call-region/README.md) owns the `Rc<CallFrame>`
contract. Where those docs describe machinery this doc assigns to the
library, this doc states the target boundary and they describe the
mechanics.

## Vocabulary

Terms used throughout, defined once. Type names marked *(working name)* fix
a concept, not a final identifier.

- **Region** — a bump-allocated arena owning stored values, with typed
  sub-arenas and the Drop discipline described in
  [per-node-memory.md](per-node-memory.md).
- **Region owner** — the handle whose drop tears a region down. Holding it,
  or a handle derived from it, is proof of liveness.
- **Witness** — a value whose possession pins a region alive at a fixed
  address. A borrow into a region is only handed out alongside a witness.
- **Brand** — a `for<'b>` closure lifetime used as an unforgeable tag: a
  reference issued at brand `'b` cannot escape the closure that introduced
  `'b`. The substrate's construction surface
  ([witnessed.rs](../workgraph/src/witnessed.rs)) is built on this device.
- **Carrier** — a stored value bundled with its witness (`Witnessed`), or
  its storable, reopenable form (`Sealed`). A carrier is born at the
  allocation site already naming everything that keeps it alive.
- **Reach set** — *(working name `RegionSet`)* an **opaque library type**
  naming the set of regions a stored value's borrows can reach. Only the
  library mints one — from region handles and carriers — so a reach set
  always represents the true union; no caller can assert or assemble one by
  hand. Reach sets are **region-hosted**: each set is stored frozen in a
  region's witness-set sub-arena and carriers hold references to it, per
  [witness-hosting.md](witness-hosting.md), which owns the representation,
  the resident/walking carrier forms, and the pinning invariant.
- **Slot / node** — one unit of scheduled work with an identity (`NodeId`),
  dep edges, and eventually a terminal.
- **Dep** — a producer another slot waits on. **Park** deps are
  notify-only (kept alive); **owned** deps are cascade-freed with their
  consumer.
- **Terminal** — a slot's finished result: a sealed carrier, or the
  workload's error.
- **Delivery envelope** — *(working name `Delivered`)* a walking terminal's
  sealed carrier paired with the producer's retained frame owner `Rc`. The
  carrier itself is **reference-only** (pins nothing); the envelope's retained
  host is what keeps its reach alive in flight, and the only verb that
  materializes a residence host into a minted set. A consumer receives one
  through `dep_delivered`; relocations ride it too, so a bare frame pin never
  escapes the scheduler.
- **Finish** — the continuation a consumer runs once its deps resolve.
- **Workload** — the embedder-facing trait: the cell contract
  ([workcell.md](workcell.md) — the continuation family, the frame owner
  `Frame`, and the brand-indexed value family Koan instantiates with
  `Carried`) plus the terminal error type the DAG layer's `Result`-shaped
  terminal protocol adds. Embedder details the scheduler would only store
  and hand back — Koan's lexical-position payload, its declared-return
  checker, its per-call semantic shell — are continuation captures, not
  trait types.

## The boundary

**The library owns:**

- The scheduling core: slots, dep edges, notify wakeups, work queues,
  splicing and alias resolution ([src/scheduler/](../workgraph/src/scheduler.rs)).
- **Regions, wholesale**: arenas, region owners, liveness. The generic
  region engine ([witnessed/region.rs](../workgraph/src/witnessed/region.rs))
  is library code, including the per-family storage bundle: a workload
  declares only its family list (`FamilyList`, a `(K, Rest)` cons-list), and
  the library derives and owns the arena bundle from it — one `FamilyArena`
  cell per family, keyed by `Stored::cell` through a tuple-field path so a
  wrong binding is a compile error rather than a runtime bug. The allocation
  capability itself is a library type,
  [`RegionHandle`](../workgraph/src/witnessed/region.rs): the engine's
  `alloc` / `alloc_resident` are `pub(crate)` to `workgraph`, so a bare
  `&Region` has no allocation surface at all — the only public minter is
  `RegionHandle::from_owner`, gated on the (unsafe-to-implement) `RegionOwner`
  contract. [arena.rs](../src/machine/core/arena.rs) holds only Koan's
  profile (`KoanStorageProfile`, `KoanRegion`, `FrameSet`, `CallFrame`) and a
  thin `RegionBrand` veneer over `RegionHandle` adding Koan-family-typed
  `alloc_*` wrappers, carrying no capability rule of its own; it allocates
  through the generic engine via the `RegionOwner` seam (the `Rc<F>` blanket
  impl that lets a foreign region-owner type pick up the library's
  `WitnessRegion`).
- The witnessed substrate ([witnessed.rs](../workgraph/src/witnessed.rs)): brands,
  carriers, erase-store, reattach.
- The reach set, as an opaque type
  ([witnessed/region_set.rs](../workgraph/src/witnessed/region_set.rs); see
  Vocabulary).
- Terminal storage and delivery: sealing results into slots, handing dep
  terminals to finishes, and the first-errored-dep short-circuit.
- The consumer API: `producer_disposition`, the `Deps` builder, the `Await`
  envelope, and the step construction context (all below).

**Koan keeps:**

- Value shape: `KObject`, `KType`, and the `Carried` family that
  instantiates the workload's value family.
- The `Action` currency
  ([action.rs](../src/machine/core/kfunction/action.rs)) and the builtin
  protocol combinators above it.
- `Scope` as a **naming layer**: lookup, binding, and shadowing semantics.
  A scope's storage is allocated through library region handles; the scope
  itself owns no arena.
- `CallFrame`: per-call lifecycle semantics. A frame **holds library region
  handles**, which is how Koan allocates objects, types, and scopes at
  will, outside any scheduler step.
- Reach **policy**: which regions a lexical chain reaches, what pins what.
  Policy code queries the opaque reach set through library predicates; it
  never constructs or decomposes one.

## The guarantees

What "safe and sound at the exported surface" means, concretely. Each
guarantee names its enforcement, because enforcement-by-type rather than
by-convention is the point.

1. **Liveness.** A stored value is only readable while its region is
   provably alive. *Enforced by:* every read goes through a carrier, and a
   carrier cannot exist without its witness.
2. **Reach totality.** A reach set always names every region the value's
   borrows can reach. *Enforced by:* the type is opaque and mintable only
   by the library, from the region handles and carriers involved in the
   allocation itself.
3. **Co-location.** A carrier is born at its allocation site, already
   witnessed; there is no "allocate bare, wrap later" path. *Enforced by:*
   the library's alloc combinators are the only constructors.
4. **Step liveness.** During a step, the scheduler itself holds the
   consumer's region owner, so the step context's region access is
   infallible — no caller-side liveness upgrade, no failure path.
   *Enforced by:* the step loop's ownership, not the caller.
5. **Escape prevention.** A dep's payload is viewable only at a closure
   brand inside the step context. Embedding it in an output value is only
   possible through the combinator that received that dep's carrier — which
   folds the dep's reach into the output's reach set as a side effect of
   the call shape. Forgetting to name a reach is not expressible.
   *Enforced by:* brands.

## Two currencies, one lowering

The library and the embedder each speak their own currency, and exactly one
place translates.

- **Library currency** (workload-generic): slots, `Deps`, `Await`
  envelopes, finishes over dep terminals. Nothing in it names a Koan type.
- **Koan currency**: [`Action`](../src/machine/core/kfunction/action.rs) —
  `Done` / `Tail` / `AwaitDeps` / `Catch` — the scheduler-agnostic shape a
  builtin returns, plus dispatch's `Outcome` on the execute side.
- **The lowering**: the action harness
  ([runtime.rs](../src/machine/execute/runtime.rs)) and the apply side are
  the only code that translates Koan currency into library envelopes.

The governance rule, stated so it can be enforced in review: **builtins
speak `Action` and the protocol combinators; dispatch internals speak the
library's consumer API; only the harness/apply side constructs raw
envelopes.** The library's envelope constructors are not visible above the
harness.

This split is load-bearing for extraction: the library compiles with no
Koan types in scope, and Koan's semantic layers never reach into scheduler
internals.

## The consumer API

Working names throughout; shapes are the commitment, identifiers are not.

**Disposition — one owner for "can I depend on this producer?"**

```rust
enum ProducerDisposition<'a, E> { Errored(&'a E), Ready, Cycle, Park }
fn producer_disposition(&self, producer: NodeId, consumer: Option<NodeId>)
    -> ProducerDisposition<'_, E>
```

The single implementation of the ready / already-errored / would-cycle /
must-park classification. Callers keep only their per-site `Ready` policy.
`consumer` is `None` at a leaf-park site with no consumer id in scope, where a
cycle can never be classified.

**`Deps` — the dep-list builder.**

```rust
let mut deps = Deps::new();
deps.park_on(producer);                    // dedup'd notify-only edge
let arg = deps.own(request);               // owned edge, returns owned index
```

`Deps` owns the `[park..., owned...]` layout internally. A finish addresses
results through a `DepResults` view — `park(i)` / `owned(j)` accessors — never
by arithmetic over a shared vector.

**`Await` — the envelope builder.**

```rust
Await::on(deps)
    .error_frame(frame)              // label attached if a dep errors
    .finish_terminal(|ctx, terminals| ...)  // reads un-relocated dep terminals
// or, for a construction that folds its deps into one witnessed carrier:
Await::on(deps).finish_witnessed(|ctx, terminals| ...)
```

The sole constructor of a parked continuation, over either finish channel.
Error short-circuit is built in through one shared walk: a finish never sees
an errored dep. Dep delivery is the terminal channel only — no bare
pre-relocated value handoff: a finish reads each dep's step-brand value and
reach carrier un-relocated. A dep whose value must outlive the resolving step
travels as its **delivery envelope**
([`Delivered`](../workgraph/src/witnessed/delivered.rs)) — the sealed carrier
paired with the producer's retained frame owner
([`Scheduler::dep_delivered`](../workgraph/src/scheduler.rs)) — so the value
stays in its producer's region and the consumer adopts it at its own step
brand under a pin sourced from the retention hold, folding the reach that pins
it, never as a relocated copy. Every delivery, including the catch channel, is
envelope-only; no dep crosses to a finish as a pre-copied value.
The one structural copy that remains, `copy_carried`
([lift.rs](../src/machine/execute/lift.rs)), is not a delivery at all — it is
the fold callback the envelope transfer
([`Delivered::transfer_into`](../workgraph/src/witnessed/delivered.rs)) runs
*inside* the two storage-bound folds that still need one:
`KoanRuntime::relocate_terminal`'s `Forward`-ready pull / run-root drain, and
`park_on_literal`'s literal fold. Both run the copy at the destination brand as
part of assembling a result carrier, so the copy's reach is folded into that
carrier's witness by construction — no pinless copy is expressible outside a
witnessed fold.

**The step construction context** ([`StepContext`](../workgraph/src/witnessed/step_ctx.rs)).
What a finish receives and the only way it can build a result:

```rust
ctx.region()                       // the consumer's live region — infallible
                                   // (guarantee 4)
ctx.alloc(|b| value)               // reach = own region only: purity is
                                   // structural, not asserted
ctx.alloc_with(&[dep_a, dep_b],    // reach = own region ∪ those deps' reaches
    |b, views| value)              // dep payloads viewable only inside, at
                                   // brand `b` (guarantee 5)
dep.carrier()                      // the dep's sealed carrier, freely
                                   // passable — for policy work
```

A finish gets **both** brand-confined payload views (for construction) and
the deps' sealed carriers (for policy: binding results into scopes,
threading argument carriers onward). Views cannot escape; carriers can,
safely.

The bare `alloc`/`alloc_with` above are the doctest-bearing conceptual
surface. Koan itself allocates through the profile-typed `_handle` veneers —
[`alloc_handle`/`alloc_with_handle`](../workgraph/src/witnessed/step_ctx.rs),
called from [arena.rs](../src/machine/core/arena.rs) — which thread the
`RegionHandle` capability the same way; both forms stay `pub` as distinct
layers, not a naming inconsistency to resolve.

**Two allocation modes, one substrate.** The step context is the
maximally-checked path. Outside a step, an embedder allocates through a held
[`RegionHandle`](../workgraph/src/witnessed/region.rs) — the `yoke` /
`yoke_handle` / `merge` construction surface of
[per-node-memory.md](per-node-memory.md) — with the same carrier and
reach-set types. In Koan: `CallFrame` holds the handle (wrapped in the
`RegionBrand` veneer); `Scope` allocates through it.

## Koan above the library

The Koan-side layers this design assumes, so the north star reads as one
picture:

- **`Action` is complete over its lowering.** Every `Action::Tail`
  placement/entry combination the dispatch layer needs is expressible, so
  dispatch hands tails to the one harness lowering rather than lowering by
  hand.
- **Protocol combinators** own the recurring builtin shapes above
  `Action`: resolve-a-type-or-await-its-producer (with the re-resolve-on-
  wake step inside — [resolve_or_await.rs](../src/builtins/resolve_or_await.rs)),
  schedule-an-aggregate-literal, and mint-a-child-scope-then-await-its-body
  (dispatch the body block against the child as an `InScope` dep, then run a
  finish that seals the child first when the caller asks for that —
  [await_body.rs](../src/builtins/await_body.rs)). A builtin states *which*
  protocol it is, not the protocol's moving parts.
- **Scope binding folds reaches through carriers.** Binding a value into a
  scope takes the value's carrier and unions its reach set into the
  scope's — policy code composing library values, never inspecting them.

## Open work

- [Consumer mints ride the delivery envelope](../roadmap/scheduler_library/consumer-envelope-mint.md)
  — makes the envelope the sole consumer-side mint verb.
- [Scheduler-owned frame storage](../roadmap/scheduler_library/scheduler-owned-frame-storage.md)
  — moves frame ownership into scheduler slot state; collapses the trait's
  memory types to `Frame` and folds the payload into continuation captures.
- [Return contracts ride continuations](../roadmap/scheduler_library/contract-as-continuation.md)
  — dissolves the stored contract type into continuation captures.
- [Carving the workcell crate](../roadmap/scheduler_library/workcell-extraction.md)
  — the crate split beneath the DAG layer.
- [Publishing the workgraph crate](../roadmap/scheduler_library/workgraph-extraction.md)
  — names, docs, and publish metadata once the boundary stops moving.
