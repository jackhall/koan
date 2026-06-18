# Scheduler owns the carrier reattaches

Make the scheduler the sole owner of the lifetime-reattach for all three erased
inter-node carriers — value, continuation, contract — and rename `cont` →
`continuation`.

**Problem.** The scheduler stores three erased carriers per node but owns the
reattach for only one. `Workload::Value` is stored `Erased<W::Value>` and
re-anchored by the scheduler on read
([`node_store.rs`](../../src/scheduler/node_store.rs)), borrow-checked end to end
against the read's own `&self`. The continuation and contract are instead stored
*opaque* and pre-erased by the workload, with the `unsafe` reattach living in the
driver: [`run_loop.rs:92`](../../src/machine/execute/run_loop.rs) reattaches
`erased_cont` to a *free* `'_`, soundness asserted by a SAFETY comment that the
step guard holds the cart; [`finalize.rs:51`](../../src/machine/execute/finalize.rs)
reattaches the contract against `post.prev_frame`. Two costs: the unsafe surface
straddles the scheduler/driver boundary instead of concentrating in the scheduler,
and the continuation fabricates a free lifetime guarded by prose rather than a
`'step` bound the compiler checks.

**Acceptance criteria.**

- `Workload::Continuation` and `Workload::Contract` are `Reattachable` families
  (mirroring the existing `type Value: Reattachable`); the scheduler stores each as
  `Erased<…>` and is the only site of their reattach.
- The continuation is vended by a scheduler accessor at a `'step` lifetime
  **bounded by a witness `&Rc<W::Frame>` the caller passes** — not a free `'_`. No
  `unsafe` continuation reattach remains in `run_loop.rs`.
- The contract is vended by a scheduler Done-boundary accessor bounded by the
  witness frame borrow the caller passes; no `unsafe` contract reattach remains in
  `finalize.rs`.
- No reattach introduces a new `Rc<W::Frame>` clone: every witness is a borrow of a
  frame `Rc` the caller already owns, so the cart strong-ref count — and therefore
  `try_reset_for_tail`'s `Rc::get_mut` TCO uniqueness gate — is unchanged.
- The continuation field and every type/local spelled `cont` are renamed
  `continuation` (`NodeWork.cont`, `erased_cont`, `ErasedCont`, …).

**Directions.**

- *Three carriers, one discipline — decided.* Value already works this way;
  continuation and contract join it as `Reattachable` families stored `Erased` on
  the lifetime-free slot, re-anchored by the scheduler witnessed by the slot's frame
  `Rc`.
- *Witness-by-borrow, never clone — decided.* TCO's ping-pong reset
  (`try_reset_for_tail`) gates on the reserve cart being uniquely owned; today
  `take_for_run` gives the driver the cart's sole strong ref for the step. Each
  accessor takes a `&Rc<W::Frame>` the caller already holds, so it adds no strong
  ref.
- *Continuation witness comes after guard entry — decided.* Today the reattach
  (`run_loop:92`) precedes the cart's move into the ambient step guard (`:93`), so
  there is no `&Rc` to borrow yet. Reorder to enter-guard → vend-continuation
  (witnessed by `ambient.active_frame_ref()`, the `&Rc<CallArena>` the guard now
  holds) → call. The vend's `'step` ends at the call, before the step's first
  `&mut self.sched` (`reclaim_deps`), so it composes.
- *Keep `take_for_run` moving the node out — decided.* The accessor takes the
  witness as a parameter rather than retaining the node in the scheduler; retaining
  it would add a second slot-side owner and force an ambient clone — reintroducing
  exactly the TCO contention the witness-by-borrow rule avoids.
- *Contract has no ordering wrinkle — decided.* `finalize.rs:51` already reattaches
  against `post.prev_frame`, a `&Rc` borrow the post-step token owns; the
  Done-boundary accessor takes that same borrow.
- *Rename `cont` → `continuation` — decided.* Spell the word out, across the field,
  the erased type alias, and locals.
- *`erase_store` vs `retype` dedup — deferred.* The audit also found
  `storage_frame::erase_store` is a second copy of the scheduler's `retype`
  primitive. That is *arena* store-side erasure in the `machine/core` substrate — a
  different axis from the inter-node carriers this item unifies — so it is left to a
  separate item, not folded into this one's criteria.

## Dependencies

Builds on the shipped `Erased<T>` / `Reattachable` owner (roadmap README, *Unified
erase/reattach carriers*). Update
[design/memory-model.md § Arena lifetime erasure](../../design/memory-model.md#arena-lifetime-erasure)
— the value-channel paragraph — when it lands.

**Requires:** none — engine-internal; the `Erased` owner already exists.

**Unblocks:** none tracked yet.
