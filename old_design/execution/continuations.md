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
composed onto the closure **generically** at the construction site: each combinator takes
a finish and returns one at its tier's own bound — `Fn + Copy` in and out for the bumped
tier ([`gated`](../../src/machine/execute/outcome.rs), `sealed_done`), `FnOnce` for the
boxed twin (`gated_once`) — so a combinator layer costs monomorphized code, not an
allocation. The closure is erased exactly once, at the install envelope.

Out-of-band step state is not a wrapper layer either. It rides beside the erased closure
as a [`ParkState`](../../src/machine/execute/obligation.rs): the slot's declared-return
`ReturnObligation` as `Copy` data, plus the block frame a leading-carrying tail keeps
alive across its park. The step deposits that state into the ambient slot-step context
before the closure runs, and a park carries the whole of it across the dormancy — so
neither a declared-return checker nor a live block frame costs a wrapping closure or an
owning capture. The frame half is a deposit/take pair, the decide depositing immediately
before it returns its park and the finish that park wakes taking it back out; a
`debug_assert` on the deposit pins that one-for-one pairing.

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
  The owning captures are enumerable and few: a pre-errored slot's `KError`
  (`decide_error`), the ambient-chain submission's one-shot finish
  ([`Host::awaiting`](../../src/machine/execute/harness.rs)), and the await half of the
  builtin `Action` tier (below). Nothing on the steady execute path reaches this tier.

Both tiers cross the same seal against the slot's anchor
([per-node-memory.md](../per-node-memory.md)); the tier decides only where the bytes
live and whether Drop runs.

## Co-location: which region hosts a bumped continuation

The region of **the frame the work installs under** — the current frame for an
in-place replace or a park, the fresh cart for a body-enter `FreshTail` (the caller's
frame dies under TCO reuse, so its region cannot host the callee's continuation). The
rule is a construction-site property, not a convention: a replace rides as one
[`Replacement`](../../src/machine/execute/outcome.rs) value bundling the work with the
frame placement, and its `fresh_tail` / `fresh_child` constructors mint the host brand
off the very frame the placement installs, so work hosted in a sibling cart's region is
unrepresentable. The closure follows the same rule as its captures, so both are covered
by the anchor's one seal. Frame-region bumps free at frame death, not at wake; a slot
parks a bounded number of times, so a dormant continuation's bytes are bounded per
frame.

## Capture discipline

- A capture that is a list of `Copy` elements (staged part indices, operator runs,
  operand spans, aggregate rows, threaded symbols, leading statements) is a bump slice
  in the closure's own region, never an owned `Vec`.
- An owning handle whose referent an existing channel already carries is re-derived at
  wake, never captured: the body-enter cart off the slot's anchor, a finish's lexical
  chain off the ambient node payload, a leading-carrying tail's block frame off the park
  state, a fresh-cart tail's installed cart off the wake-time view.
- A nested closure currency (a field-list composer, an aggregate assembler) is a generic
  parameter folded into the finish before the one erasure, not a second box inside it:
  `Fn + Copy` where the finish is bumped, `FnOnce` where it rides the boxed `Action` tier.

## The builtin tier

Builtin bodies hand the engine their wake logic through the `Action` currency
([action.rs](../../src/machine/core/kfunction/action.rs)) — an open composition surface
where each builtin writes its own closure. The engine adds no wrapper of its own: the
`FinishCtx` assembly and the `run_action` recursion compose in generically, at whichever
tier the builtin's own closure sits on.

- A **catch** finish (`CatchFn`) is single-tier and bumped. It erases at the builtin
  construction site into the region of the frame the park installs under; `Action::catch`
  names that host as a `RegionBrand` rather than a bare allocator, so a step-scratch host
  — dangling at the next drain pop — is unrepresentable. Both catching builtins
  ([TRY](../../src/builtins/try_with.rs), [CATCH](../../src/builtins/catch.rs)) carry
  `Copy` captures, so a recovery costs no heap allocation and there is no owning twin to
  pick between.
- An **await** finish (`AwaitContinue`) is the one builtin surface that stays boxed: the
  binder finishes riding it own their staged declaration state. It fires on no steady
  tail-loop path.

The rest of the `Action` surface carries no continuation of its own. A tail's leading
statements ride as a region slice rather than an owned `Vec`; the block seed a MATCH or
TRY arm binds `it` through is a stack `impl FnOnce` that runs before `block_tail`
returns; and the leading-statements finish the engine synthesizes reads its block frame
back off the park state instead of capturing it, so it is `Copy` and bumped like every
other engine-side finish. `ActionKind::TailRaw` — a body that crosses a cart install as
raw AST and freezes at the installed cart's brand — is the same shape: the freeze
closure is the engine's, hosted in the cart the replace installs and carrying only
`Copy` captures.

The step's binding writes ride the same discipline: a body's `WriteOp` run is a
`BumpVec` on the step arena, minted only once a body actually decides a write and handed
back when the drain pops. See
[classify-and-apply.md § The step's binding writes](classify-and-apply.md#the-steps-binding-writes).
