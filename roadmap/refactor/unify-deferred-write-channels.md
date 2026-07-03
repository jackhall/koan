# Unify the three deferred-write channels

**Problem.** The value / function / type deferred-write channel is spelled once per
channel in three files that must stay in sync. `src/machine/core/scope.rs`
(`bind_value`, `register_function`, `register_type`) each run the same
`try_* → Applied / Conflict → pending.defer_*` shape; `src/machine/core/pending.rs`
carries one `PendingWrite` variant per channel and `drain` (lines 117-181) re-matches
all three — destructuring a conflicted item and rebuilding the same variant just to
requeue it; `src/machine/core/bindings.rs` supplies the per-channel writers
(`try_bind_value`, `try_register_function`, `try_register_type`). Adding a fourth
channel, or changing conflict semantics, touches all three files in parallel.

**Acceptance criteria.**

- Each write channel is described in one place — its payload, its deferred form, and
  its replay routing — so adding a channel is a one-site change.
- `PendingQueue::drain` requeues a conflicted item without destructuring and rebuilding
  it.
- Deferred writes still replay through the same validated `Bindings` write path as
  direct writes (function-mirror invariant, per-map collision checks intact); existing
  tests green.

**Directions.**

- *Fold shape — open.* (a) A `WriteOp` value (name + per-channel payload + index +
  reach) with one `apply(&Bindings) -> Result<ApplyOutcome, KError>` method; the
  `Scope` methods build it, `PendingQueue` stores `Vec<WriteOp>`, and `drain` becomes
  one loop over `apply`. (b) Queue boxed `FnOnce(&Bindings)` closures. Recommended:
  (a) — keeps the channel tag inspectable (the current docs call the variant tag
  load-bearing for routing per-map collision checks) and avoids a per-deferral
  allocation.
- *Public surface — open.* Keep the three public `Scope` / `Bindings` methods as thin
  wrappers, or expose the `WriteOp` at the `Scope` boundary too. Recommended: keep the
  wrappers — call sites stay unchanged and the fold is internal.

## Dependencies

Lands cleanest after [Fold the two type-write paths](fold-type-write-paths.md), so the
channel fold wraps one type writer instead of two — soft ordering, not a prerequisite.

**Requires:** none — leaf cleanup.

**Unblocks:** none tracked.
