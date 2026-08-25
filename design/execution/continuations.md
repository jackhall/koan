# The continuation currency

How a node's "what happens next" is represented, composed, and stored. The scheduler
stores one continuation per slot ([`NodeWork`](../../workgraph/src/scheduler/nodes.rs))
and treats it as opaque; this page is the koan side's shape for it —
[scheduler.md](scheduler.md) carries the step loop that runs it, and
[classify-and-apply.md](classify-and-apply.md) the decides that build it.

## One signature, one erasure

Every stored continuation is a closure over
`(&DecideCtx, &[Result<DepTerminal, KError>], NodeId) -> Outcome`. A decide takes no
dep values and ignores the slice; a dep-finish reads it; nothing else exists. What
distinguishes a finish's delivery — the dep-error short-circuit and its deferred trace
frame, a witnessed fold's `Result` projection, a catch's no-short-circuit read — is
composed onto the closure **generically** at the construction site (`impl FnOnce` in,
`impl FnOnce` out), so a combinator layer costs monomorphized code, not an allocation.
The closure is erased exactly once, at the install envelope.

The slot's declared-return obligation is not a wrapper layer: it rides beside the
erased closure as `Copy` data
([`ReturnObligation`](../../src/machine/execute/obligation.rs)), deposited into the
ambient slot-step state by the step before the closure runs. A park re-carries the
ambient obligation the same way, as data.

## The two-tier erase door

- **Bumped** — a `Copy` closure is bump-allocated into a frame region (the
  [`BumpAllocator::value`](../../workgraph/src/witnessed/bump.rs) `T: Copy` guard) and
  stored as a `&dyn` call target, on the Drop-free reattachable tier. This is the
  default and covers the whole steady execute path. A `Copy` closure is one whose every
  capture is `Copy` — the Drop-free region invariant as a compile fact: a continuation
  that never runs (deadlock teardown, an error unwinding parked slots) is plain bytes,
  freed with the region's chunks.
- **Boxed** — a closure with an owning capture takes `Box<dyn FnOnce>` and the
  droppable reattachable tier, exactly so its captures' destructors run at slot death.
  The owning captures are enumerable: the leading-statements finish's block-frame `Rc`
  (load-bearing — nothing else keeps the block frame alive across that park), a
  pre-errored slot's `KError`, a catch's recovery closure, and every builtin `Action`
  finish (below).

Both tiers cross the same seal against the slot's anchor
([per-node-memory.md](../per-node-memory.md)); the tier decides only where the bytes
live and whether Drop runs.

## Co-location: which region hosts a bumped continuation

The region of **the frame the work installs under** — the current frame for an
in-place replace or a park, the fresh cart for a body-enter `FreshTail` (the caller's
frame dies under TCO reuse, so its region cannot host the callee's continuation). The
closure follows the same rule as its captures, so both are covered by the anchor's one
seal. Frame-region bumps free at frame death, not at wake; a slot parks a bounded
number of times, so a dormant continuation's bytes are bounded per frame.

## Capture discipline

- A capture that is a list of `Copy` elements (staged part indices, operator runs,
  operand spans, aggregate rows, threaded symbols, leading statements) is a bump slice
  in the closure's own region, never an owned `Vec`.
- An owning handle whose referent an existing channel already carries is re-derived at
  wake, never captured: the body-enter cart off the slot's anchor, a finish's lexical
  chain off the ambient node payload.
- A nested closure currency (a field-list composer, an aggregate assembler) is an
  `impl FnOnce` folded into the finish before the one erasure, not a second box inside
  it.

## The builtin tier

Builtin bodies hand the engine their wake logic through the `Action` currency
([`AwaitContinue` / `CatchContinue`](../../src/machine/core/kfunction/action.rs)) — an
open composition surface where each builtin writes its own closure. Those arrive boxed
and take the Boxed tier; the engine adds no wrapper of its own (the `FinishCtx`
assembly and the `run_action` recursion compose in generically).

## Open work

- **Body-enter path**
  ([roadmap/reduce_allocs/body-enter-continuation.md](../../roadmap/reduce_allocs/body-enter-continuation.md)):
  the fresh-cart co-location, the re-derived cart, the leading statements as a region
  slice.
- **Dep-finish captures**
  ([roadmap/reduce_allocs/dep-finish-captures.md](../../roadmap/reduce_allocs/dep-finish-captures.md)):
  the engine dep-finish sites onto the bumped tier with region-slice captures.
- **Builtin action continuations**
  ([roadmap/reduce_allocs/builtin-action-continuations.md](../../roadmap/reduce_allocs/builtin-action-continuations.md)):
  moving the builtin `Action` surface off the Boxed tier.
