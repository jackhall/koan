# The scheduler library

Koan's runtime substrate — the deferred-work scheduler, the region memory
system, and the witnessed carrier machinery — is a self-contained library
stack with no dependency on Koan's language semantics. It ships as two
workspace crates: `cellgraph` *(working name — [cellgraph.md](../workgraph/design/cellgraph.md))*,
the computation-cell substrate (witnessed memory plus a cell table:
continuations, memory anchors, inter-cell values — no acyclicity, no
terminality), and `workgraph`, the DAG scheduler layered on it (dep edges,
wake/notify, cycle detection, terminal delivery, splicing). The
dependency direction (`koan` → `workgraph` → `cellgraph`, never the reverse)
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
[witnessed-memory.md](../workgraph/design/witnessed-memory.md) owns the
witnessed substrate mechanics and
[reach.md](../workgraph/design/reach.md) the reach representation;
[per-node-memory.md](per-node-memory.md) and
[witness-hosting.md](witness-hosting.md) own Koan's instantiation and policy
over them; [execution/](execution/README.md) owns the pipeline;
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
  [witnessed-memory.md](../workgraph/design/witnessed-memory.md).
- **Region owner** — the handle whose drop tears a region down. Holding it,
  or a handle derived from it, is proof of liveness.
- **Witness** — a value whose possession pins a region alive at a fixed
  address. A borrow into a region is only handed out alongside a witness.
- **Brand** — a `for<'b>` closure lifetime used as an unforgeable tag: a
  reference issued at brand `'b` cannot escape the closure that introduced
  `'b`. The substrate's construction surface
  ([witnessed.rs](../workgraph/src/witnessed.rs)) is built on this device. The
  step open bounds its brand from above with a
  [`Within<'b, 'outer>`](../workgraph/src/witnessed/dormant.rs) token, so an
  embedder capability held as a live borrow for the whole run — Koan's
  `ProgramBrand` is the one in use — reaches step code at `'outer` without
  becoming a carrier: the token's declared `'outer: 'b` discharges the
  `'outer: 'b` bound the step-side struct holding it declares
  ([witnessed-memory.md](../workgraph/design/witnessed-memory.md)).
- **Carrier** — a stored value bundled with its witness (`Witnessed`), or
  its storable, reopenable form (`Sealed`). A carrier is born at the
  allocation site already naming everything that keeps it alive.
- **Reach set** — an **opaque library pair** naming the set of regions a
  stored value's borrows can reach: a non-owning **description**, stored
  frozen in a region-owned table with carriers holding references to it, and
  an owned **pin bundle** carrying the strong region holds. Only the library
  mints one — from region handles and carriers, always as the pair — so a
  reach set always represents the true union; no caller can assert or
  assemble one by hand. [reach.md](../workgraph/design/reach.md) owns the
  representation, the resident/walking carrier forms, and the holder rule.
- **Slot / node** — one unit of scheduled work, with a crate-internal
  identity; the embedder never holds a node.
- **Edge** — one consumer→producer relationship, the sole boundary currency
  (`EdgeId`): the embedder wires parked deps, dispatch placeholders, scope
  bindings, and the run's roots alike through `EdgeId`s — names granting the
  library's wiring and read verbs, never ownership of the edge. An edge is a
  wake entry while the producer is pending and holds the delivered resident
  after; it is valid until its owner (a consumer node, or the frame whose
  teardown carries the release) releases it, and every edge names the
  destination region its value is delivered into
  ([workgraph/design/dag-scheduler.md § Edges and the boundary](../workgraph/design/dag-scheduler.md#edges-and-the-boundary)).
- **Dep** — a producer another slot waits on, held as an edge. The
  **park**/**owned** labels are Koan's `Deps`-currency roles (positional
  addressing and dispatch classification), not scheduler semantics.
- **Terminal** — a slot's finished result: a sealed carrier, or the
  workload's error.
- **Delivery envelope** — *(working name `Delivered`)* a walking terminal's
  sealed carrier paired with its owned pin bundle — the producer's retained
  frame owner plus the value's foreign pins. The carrier itself is
  **reference-only** (pins nothing); the envelope's bundle is what keeps its
  reach alive in flight, and the only verb that materializes a residence host
  into a minted pair. A producer hands one to `finalize` / `rehome_terminal`;
  from there it is internal transit inside the delivery walk, which adopts the
  terminal into each edge's destination region
  ([workgraph/design/dag-scheduler.md § Delivery at finalize](../workgraph/design/dag-scheduler.md#delivery-at-finalize)).
  No envelope crosses to a consumer — deps arrive as ordinary residents — and
  a bare frame pin never escapes the scheduler.
- **Finish** — the continuation a consumer runs once its deps resolve.
- **Workload** — the embedder-facing trait: the cell contract
  ([cellgraph.md](../workgraph/design/cellgraph.md) — the continuation family, the memory anchor
  `Frame` (which projects its region owner through `Anchor::owner`), and the
  brand-indexed value family Koan instantiates with `Carried`) plus the
  terminal error type the DAG layer's `Result`-shaped terminal protocol adds.
  Koan's lexical-position payload and its per-call semantic shell ride *inside*
  the anchor — the scheduler stores and hands them back but inspects nothing
  beyond `Anchor::owner`; its declared-return checker is a continuation
  capture, not a trait type.

## The boundary

**The library owns:**

- The scheduling core: slots, dep edges, notify wakeups, work queues,
  splicing ([src/scheduler/](../workgraph/src/scheduler.rs)).
- **Regions, wholesale**: the bump, region owners, liveness. The generic
  region engine ([witnessed/region.rs](../workgraph/src/witnessed/region.rs))
  is library code, and a region's value storage is its bump and nothing else —
  a workload declares only the frame-owner type its reach descriptions are
  typed at (`StorageProfile::FrameOwner`), never a per-family storage policy.
  The allocation capability itself is a library type,
  [`RegionHandle`](../workgraph/src/witnessed/region.rs): the region's own
  `allocator` is `pub(crate)` to `workgraph`, so a bare
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
- The reach set, as an opaque description/bundle pair
  ([witnessed/reach.rs](../workgraph/src/witnessed/reach.rs); see
  Vocabulary).
- Terminal delivery: adopting each terminal into its edges' destination
  regions at finalize, and the first-errored-dep short-circuit.
- The consumer API: the install verb (filled-or-parked) and
  `would_create_cycle`, the `Deps` builder, the `Await` envelope, and the
  step construction context (all below).

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
   borrows can reach. *Enforced by:* the pair is opaque and mintable only
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

**The install door — the generic building block.**

Wiring goes through one public door, `Scheduler::install_edges`, and **install
returns filled-or-parked** per edge: *filled* means the producer had already
finalized and the value was delivered into the edge's destination at install;
*parked* means the consumer waits and the wake fires at the producer's
finalize. The "can I depend on this producer?" classification — ready /
already-errored / must-park — is install-and-inspect, not a separate probe
ladder; the would-cycle guard stays a pre-wiring query. Every arm's meaning is
a Koan dispatch decision, so the classification lives Koan-side over
`KoanWorkload` / `KError` ([dispatch.rs](../src/machine/execute/dispatch.rs)),
built on the install door. The library names no Koan type and stays smaller.

**`Deps` — the dep-list builder.**

```rust
let mut deps = Deps::new();
deps.park_on(producer);                    // dedup'd park entry
let arg = deps.own(request);               // owned entry, returns owned index
```

`Deps` owns the `[park..., owned...]` layout internally. A finish addresses
results through a `DepResults` view — `park(i)` / `owned(j)` accessors — never
by arithmetic over a shared vector.

**`Await` — the envelope builder.**

```rust
Await::on(deps)
    .error_frame(frame)              // label attached if a dep errors
    .finish_terminal(|ctx, terminals| ...)  // reads the deps as residents
// or, for a construction that folds its deps into one witnessed carrier:
Await::on(deps).finish_witnessed(|ctx, terminals| ...)
```

The sole constructor of a parked continuation, over either finish channel.
Error short-circuit is built in through one shared walk: a finish never sees
an errored dep. Deps arrive as **ordinary residents of the consumer's
region**, delivered at the producer's finalize
([workgraph/design/dag-scheduler.md § Delivery at finalize](../workgraph/design/dag-scheduler.md#delivery-at-finalize)):
the delivery adopt mints each value's reach against the destination region,
with the embedder's retention predicate deciding deepcopy vs pin per
destination — so a finish reads each dep as a resident carrier co-located with
the continuation, and no dep ever crosses as a bare value-plus-reach pair.
The one structural copy, `copy_carried`
([lift.rs](../src/machine/execute/lift.rs)), is the fold callback the delivery
adopt runs when the predicate rules a copy; it runs at the destination brand as
part of assembling the resident carrier, so the copy's reach is folded into
that carrier's witness by construction — no pinless copy is expressible
outside a witnessed fold.

**The step construction context** ([`StepContext`](../workgraph/src/witnessed/step_ctx.rs)).
What a finish receives and the only way it can build a result:

```rust
ctx.region()                       // the consumer's live region — infallible
                                   // (guarantee 4)
ctx.alloc(|handle| value)          // reach = own region only: purity is
                                   // structural, not asserted
ctx.alloc_with(&[dep_a, dep_b],    // reach = own region ∪ those deps' reaches
    |placement, views| value)      // dep payloads viewable only inside, at
                                   // the placement's brand (guarantee 5)
dep.carrier()                      // the dep's sealed carrier, freely
                                   // passable — for policy work
```

A finish gets **both** brand-confined payload views (for construction) and
the deps' sealed carriers (for policy: binding results into scopes,
threading argument carriers onward). Views cannot escape; carriers can,
safely.

There is one door per verb. `alloc` hands its closure the region's
`RegionHandle`, and `alloc_with` hands it a `FoldedPlacement` over that same
handle — the allocation capability and the fold-brand proof as one value, so
the two are never paired by hand. The region-flavoured forms that take a bare
`&Region` plus a separate `FoldToken` are crate-internal implementations of
those two, not a second public layer.

**Two allocation modes, one substrate.** The step context is the
maximally-checked path. Outside a step, an embedder allocates through a held
[`RegionHandle`](../workgraph/src/witnessed/region.rs) — the `yoke` /
`yoke_handle` / `merge` construction surface of
[witnessed-memory.md](../workgraph/design/witnessed-memory.md) — with the same carrier and
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
  (dispatch the body block against the child as an `InScope` dep, then close the
  child and run the caller's finish —
  [await_body.rs](../src/builtins/await_body.rs)). A builtin states *which*
  protocol it is, not the protocol's moving parts.
- **Scope binding folds reaches through carriers.** Binding a value into a
  scope takes the value's carrier and mints its reach pair against the
  scope's region — description into the table, pins onto the binding entry —
  policy code composing library values, never inspecting them.

## Open work

- [Koan wires through edges](../roadmap/refactor/edge-wiring-migration.md) —
  koan adopts `EdgeId`s and the install door.
- [Collapse the Deps owned/park currency](../roadmap/refactor/deps-currency-collapse.md)
  — one dep list, one index space.
- [Carving the cellgraph crate](../workgraph/roadmap/cellgraph-extraction.md)
  — the crate split beneath the DAG layer.
- [Publishing the workgraph crate](../workgraph/roadmap/workgraph-extraction.md)
  — names, docs, and publish metadata once the boundary stops moving.
