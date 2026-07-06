# Library-owned tail-call region reuse

Move tail-call region turnover onto the library's node lifecycle so Koan reuses a
slot but never manipulates a region, per
[design/tail-call-optimization.md](../../design/tail-call-optimization.md) — the
doc that carries the full reasoning; read it before planning this item.

**Problem.** TCO's heap-level lever is region manipulation Koan performs by hand.
[`try_reset_for_tail`](../../src/machine/core/arena.rs) mints a fresh `KoanRegion`
and drops the old `FrameStorage` in place; because the active frame's region is
borrowed across a synchronous invoke, the reuse target is a **two-iteration-old
reserve** that ping-pongs across `NodeStep::Replace`, held as `active_reserve` on
[`AmbientContext`](../../src/machine/execute/ambient.rs) beside `active_frame`.
This puts region lifetime in Koan's hands — Koan calls `KoanRegion::new()`, decides
when a region resets, and carries a bespoke reserve-rotation apparatus to time it
safely — which the [library-owned-regions boundary](../../design/scheduler-library.md)
forbids: the library owns regions wholesale, and Koan should touch only nodes and
deps.

**Acceptance criteria.**

- A tail call reinstalls the slot's work
  ([`reinstall`](../../workgraph/src/scheduler/node_store.rs), which already
  exists) and issues no region mint, reset, or drop; the library retires the
  retiring incarnation's region and provisions the next as a function of the
  reinstall. The reinstall applies where every graph edit applies — after the
  step returns, in [`apply_outcome`](../../src/machine/execute/runtime.rs), the
  sole graph-writing site — never mid-step.
- `try_reset_for_tail`, `active_reserve`, and the ping-pong reserve rotation no
  longer exist; `AmbientContext` carries no reserve frame.
- An incarnation's per-call region is minted lazily by the library on its first
  allocation, not at reinstall. The no-mint incarnation categories of
  [tail-call-optimization.md § Region liveness](../../design/tail-call-optimization.md#region-liveness-by-node-lifetime)
  — syntactic reductions, bare-name forwards, `USING` overlay entries,
  top-level / run producers — mint nothing.
- A depth-`N` tail loop runs on one node slot and in `O(1)` live regions (≤ 2
  transiently), with the retiring region held until the reinstalled incarnation
  adopts the carried arguments.
- Loop-carried arguments cross to the reinstalled incarnation as owned deps,
  sealed in the retiring incarnation's region and adopted through the ordinary
  carrier-delivery path
  ([`transfer_into`](../../workgraph/src/witnessed.rs)); no TCO-specific seed
  relocate remains.
- A tail chain checks the original caller's declared return; the kept-first
  contract's home region is pinned by the contract's carried witness across every
  hop, with no `FrameStorage.outer` Rc chain pinning it.
- The full Miri audit slate passes: 0 leaks, 0 UB, including the
  recursive-match use-after-free fixture
  ([fn_def/tests/arena.rs](../../src/builtins/fn_def/tests/arena.rs)) at every
  iteration.

**Directions.**

- *Reinstall with stable node identity — decided,* per
  [tail-call-optimization.md § The design](../../design/tail-call-optimization.md#the-design-reinstall-the-slot-turn-over-the-region).
  The slot is reinstalled, keeping its id, so the consumer edge is untouched — no
  forward, no `splice_forward` tombstone, no consumer re-point — and an alias
  taken mid-loop still names the same slot at loop exit. A fresh-id
  spawn+free variant (repoint the consumer, free the predecessor) is rejected: it
  needs a consumer-relabel primitive and a producer→consumer reverse lookup the dep
  graph does not have.
- *Free ordering by the arguments' witness — decided.* The retiring
  incarnation's sealed argument carriers each hold its frame-owner `Rc` (the
  [host-pinned walking carrier](host-pinned-walking-carrier.md)'s one arm), so
  when the library drops its own handle at the reinstall, the region lives
  until the reinstalled incarnation's adoption copy drops those carriers —
  the free is ordered after the copy by refcount, with no retention machinery.
  [Delivery-driven frame retention](delivery-driven-frame-retention.md) later
  re-sources this same ordering from pull-count release; the turnover mechanism
  built here does not change.
- *Lazy per-incarnation region mint — decided.* The library mints an incarnation's
  region on its first allocation, not at reinstall; `CallFrame` becomes a
  *reference* to its incarnation's library-owned region, holding no power to
  create or destroy one.
- *Region turnover is library-driven off the reinstall — decided.* Koan's reinstall
  is the trigger; the library retires the retiring incarnation's region and
  provisions the next. Koan passes no region handle and calls no reset verb.
- *Contract keep-first via the contract's carried witness — decided.* The
  kept-first return contract's home region is named by the contract's carried
  witness (a witnessed cross-region borrow), which keeps it alive across every hop;
  the keep-first `or` retains the first contract's reach alongside it. This replaces
  the [`FrameStorage.outer`](../../src/machine/core/arena.rs) Rc chain that pins an
  `Arm` contract's ancestor home region today — the only case at risk, since a
  `Function` / `PerCall` contract's home is the FN closure's captured scope, pinned
  independently. The `active_in_contract_chain` flag
  ([ambient.rs](../../src/machine/execute/ambient.rs)) and the finalize re-home
  path resolve the home region through the witnessed reach, needing no reserve or
  `outer` link.

## Dependencies

The free ordering rests on the carried arguments' witness: the sealed argument
carriers pin the retiring incarnation's region until the reinstalled incarnation
adopts them. The single host-pinned carrier those arguments ride ships first;
delivery-driven retention later moves the same ordering onto pull-count release
with no change to the turnover mechanism.

**Requires:**

- [Host-pinned walking carrier](host-pinned-walking-carrier.md) — supplies the
  single host-pinned carrier the sealed arguments and kept-first contract ride.

**Unblocks:**

- [Delivery-driven frame retention](delivery-driven-frame-retention.md) — makes
  region lifecycle library-owned, so retention lands against a single release
  mechanism.
- [Publishing the workgraph crate](workgraph-extraction.md) — removes Koan's last
  direct region manipulation, so the library/embedder boundary is clean before the
  crate's surface freezes.
