# Step-local staging on the scratch arena

**Problem.** Several buffers on the per-step and per-dispatch path are built, read, and
dropped inside one step, and each takes a heap allocation:

- `dep_sources` in `Host::step`
  ([src/machine/execute/harness.rs](../../src/machine/execute/harness.rs)) — one per step
  that has deps, re-branding each delivered resident against the step's coverage.
- `all_or_first_error`'s terminal collection
  ([src/machine/execute/outcome.rs](../../src/machine/execute/outcome.rs)) — one per
  finish. It collects `Vec<&DepTerminal>` while `TerminalDepFinish` takes `&[&DepTerminal]`
  and `DepTerminal` is a thin `Copy` wrapper, so the references buy nothing, but a
  contiguous slice still has to come from somewhere.
- `build_bare_outcomes` ([src/machine/execute/decide/ctx.rs](../../src/machine/execute/decide/ctx.rs))
  — one per keyworded dispatch, and a second time in `keyworded::finish` after eager subs land.
- `carriers_from_expr` ([src/machine/execute/decide/exec.rs](../../src/machine/execute/decide/exec.rs))
  — one per invoke.
- `retiring` in the placeholder-clearing path
  ([src/machine/execute/harness.rs](../../src/machine/execute/harness.rs)) — a
  `Vec<ProducerId>` built from the edge list purely to satisfy
  `clear_placeholders_for_producers(&[ProducerId])`, where `ProducerId::from_scheduler_edge`
  is a conversion.

Every one of them has an exactly-known length at construction, none escapes the step, and
none has a fix that does not either distort a door's signature or relocate the buffer.

**Acceptance criteria.**

- Each buffer listed above is scratch-hosted, built with its known capacity, and never
  reallocates within a step.
- `all_or_first_error` hands its finish a slice of `DepTerminal` values rather than a
  slice of references to them.
- The placeholder-clearing path takes the edge list as its currency, so no
  `Vec<ProducerId>` is built to satisfy the door.
- The recorded per-step allocation count for a tail-recursive loop is constant across
  iterations.
- Any buffer that could not move is named in `design/` with the reason it stays on the heap.

**Directions.**

- *Buffers reachable only before the step brand opens — decided.* The scratch handle
  arrives in `Host::step` as a `Step` field, disjoint from the `&mut self` borrows held
  before `sealed_continuation.open`, so `dep_sources` is arena-hosted where it is built
  today; nothing needs to move inside the open.

## Dependencies

**Requires:** none — the step scratch arena this item fans out over is shipped: the drain
owns the bump, hands its handle out on `Step::scratch`
([workgraph/src/scheduler/drain.rs](../../workgraph/src/scheduler/drain.rs)), and it reaches
decide code at `'step` through
[`DecideCtx::scratch`](../../src/machine/execute/decide/ctx.rs). The dispatch bucket walk
([src/machine/execute/decide/resolve_dispatch.rs](../../src/machine/execute/decide/resolve_dispatch.rs))
is its first consumer and the pattern the buffers below follow.

**Unblocks:** none.
