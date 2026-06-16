# Workload-independent DAG runtime

Confine the run lifetime to `KoanRuntime` by erasing per-node continuations and
evicting Koan semantics from the scheduler, leaving a generic per-node-memory DAG
runtime.

**Problem.** The scheduler is structurally entangled with Koan semantics in two
ways that keep `'run` smeared across every `scheduler/` file. First, the boxed
per-node continuation `NodeCont<'a>`
([`src/machine/execute/outcome.rs`](../../src/machine/execute/outcome.rs))
*captures* run-lived data (function AST, captured scope), so the `+ 'a` bound on the
box pins `'run` even though the continuation's *output* lifetime is already a per-step
`'s` (the scheduler lifts each dep into the consuming frame at read —
[per-call-arena-protocol.md § Consumer-pull node-output lift](../../design/per-call-arena-protocol.md#consumer-pull-node-output-lift)). Second, the
scheduler stores Koan-semantic state that does not belong to a generic DAG runtime:
each `Node` carries `scope: NodeScope<'run>` and `chain: Rc<LexicalFrame>`
([`src/machine/execute/nodes.rs`](../../src/machine/execute/nodes.rs)) alongside its
memory frame, and the scheduler keeps parallel ambient copies (`active_chain`,
`active_node_scope`) — name-resolution concepts with Koan meaning. Conversely, the
per-node *memory* abstraction the scheduler should own — `CallArena` — lives outside
it in [`src/machine/core/arena.rs`](../../src/machine/core/arena.rs). The result is a
scheduler that cannot be reasoned about or tested independently of the Koan
value/scope model.

**Acceptance criteria.**

- `'run` appears at exactly one place — the `KoanRuntime` workload boundary — and not
  across `scheduler/**`. The node-stored payload holds no `'run` data: it is
  lifetime-erased and re-anchored to the node frame lifetime when the node runs.
- A node's continuation is stored erased (no `'run` capture bound) and re-anchored
  against the node's own frame when run — the same erase / reattach discipline
  `ErasedContract` uses today
  ([`src/machine/execute/scheduler/execute.rs`](../../src/machine/execute/scheduler/execute.rs)),
  generalized from the contract to the whole continuation.
- `scope` and `chain` are no longer fields the scheduler interprets: Koan
  name-resolution state rides as opaque per-node workload payload the scheduler
  stores and hands back but never inspects.
- The per-node memory frame (`CallArena`) is owned by the scheduler module; the
  scheduler mints, reuses (TCO), and drops one memory frame per node.
- The scheduler crate-region builds and its tests pass without naming any Koan
  value, scope, or AST type; dispatch / TCO behavior and the Miri slate are
  unchanged.

**Directions.**

- *CallArena relocation — decided.* The scheduler becomes the per-node memory manager —
  it mints, reuses (TCO), and drops one memory frame per node. The frame wraps the generic
  `StorageFrame` storage substrate (see *Storage substrate* below), which names no Koan type;
  the frame wrapper itself (`CallArena`, holding the erased scope payload) genericizes alongside
  the payload eviction, so the scheduler ends up managing per-node memory without naming a Koan
  type while the model→frame back-edge keeps compiling. "Owned by the scheduler" means exclusive
  *manager*, not definer.
- *Scope-handle erasure — decided.* The `NodeScope::Anchored(&'run Scope)` borrow is
  the lifetime carrier that blocks a lifetime-free frame; folding it in is a
  dealbreaker. Store an erased scope pointer instead — the `ScopePtr<'static>`
  mechanism `CallArena` already uses for per-call scopes
  ([`src/machine/core/scope_ptr.rs`](../../src/machine/core/scope_ptr.rs)) —
  re-anchored at read, not a live `&'run` borrow.
- *Payload carriage — decided.* The scheduler is generic over **two** workload type
  parameters: a node-stored payload (persisted across a slot's steps — Koan: `scope`,
  `chain`, `contract`, continuation) and an inter-node value passed along dep edges
  (Koan: the lifted `Carried`). Both are carried lifetime-erased so the scheduler holds
  no workload lifetime. The per-node frame lifetime the scheduler manages is a *distinct*
  lifetime, not folded into either type parameter — a node's payload / value is
  re-anchored to it at run / read time. Lift still erases lifetimes (the inter-node value
  is erased out of the producing frame and re-anchored on delivery — the shipped
  consumer-pull lift, see
  [per-call-arena-protocol.md § Consumer-pull node-output lift](../../design/per-call-arena-protocol.md#consumer-pull-node-output-lift)).
- *Lift hook — decided.* The lift policy / mechanism split is the shipped `NodeLift`
  workload hook ([`src/machine/execute/lift.rs`](../../src/machine/execute/lift.rs)); this
  item consumes it generically rather than redefining it.
- *Storage substrate (`StorageFrame`) — decided; the first slice.* The per-call allocator
  genericizes first, independently of the payload/continuation work. A generic `StorageFrame<W>`
  — the run-lifetime erase-store substrate (the irreducible `unsafe`) plus the `escape`
  cycle-redirect pointer and the address membership side-table — lives in a low `core` submodule
  and names no Koan type. A generic `Stored<W>` trait (today's `ArenaStored`, lifted off the
  concrete families) carries each family's `At<'a>` projection, its `sub_arena`, and its required
  `anchors_to` gate answer; the single private `alloc<K>` engine runs the cycle gate by calling
  `anchors_to` for every family. Unbypassability comes from the substrate's *private* `storage`
  field and that single store path — not from sealing, so `Stored` is an open extension point the
  workload implements and no `&Arena` is ever exposed (see
  [per-call-arena-protocol.md § Cycle gate](../../design/per-call-arena-protocol.md#cycle-gate-on-alloc_object)).
  `RuntimeArena` becomes `StorageFrame<KoanWorkload>` — a transparent alias, so the ~676
  `RuntimeArena` sites are untouched — and `CallArena` keeps wrapping it unchanged; the Koan
  instantiation (the storage bundle, the `Stored` impls, the cycle-gate walkers) stays in
  `core::arena`. Because `Scope` embeds `&RuntimeArena`, that instantiation must stay nameable
  from `core`, so the `core::arena → model` edge persists; severing it needs the slice-2
  `Scope`-side erasure. The win here is the Koan-agnostic substrate and the generic-once `unsafe`,
  not an edge inversion.

## Slices

1. **Storage substrate (first slice).** Introduce the generic `StorageFrame` + `Stored`
   trait in a low `core` submodule; re-express `RuntimeArena` as the Koan instantiation
   `StorageFrame<KoanWorkload>` (a transparent alias), with `CallArena` wrapping it unchanged.
   No change to `scope` / `chain`, the continuation, or the back-edge, and the
   `core::arena → model` edge persists (the Koan instantiation stays in `core`).
   Independently shippable: `RuntimeArena` is owned by `CallArena` in `core`, so genericizing
   storage touches neither name-resolution state nor the scheduler's `'run`.
2a. **Scope/chain payload eviction.** Evict `scope` / `chain` off the node into an opaque
   per-node payload the scheduler stores but never interprets, and erase the one remaining live
   scope borrow — `NodeScope::Anchored(&'run Scope)` becomes an erased `ScopePtr` re-anchored at
   read. A precursor makes the return contract self-describing (it carries its own re-tag home
   arena, witnessed by the cart `Rc`), so the Done boundary's `enforce_return_contract` reads no
   scope. Independently shippable: removes the live borrow and relocates scope/chain interpretation
   to the workload boundary while `'run` still rides the continuation.
2b. **Continuation erasure + scheduler genericization (remainder).** Store the continuation erased
   (no `'run` capture, reattached against the node frame at invoke), turn the Done-boundary
   contract enforcement into a workload finalize hook, make the scheduler generic over the two
   workload type params, and consume `NodeLift` generically — collapsing `'run` to the
   `KoanRuntime` boundary. The model→frame back-edge erases here, if at all.

## Dependencies

This is the second half: the continuation's *output* lifetime has already been
shrunk to a per-step `'s` behind the shipped `NodeLift` hook (consumer-pull delivery);
this item erases its *captures* and evicts the remaining Koan-semantic state so `'run`
collapses to the run frame `KoanRuntime` holds.

**Requires:** none — the output-lifetime/lift-hook prerequisite has shipped.

**Unblocks:** none tracked yet.
