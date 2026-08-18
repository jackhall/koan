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
- `split_working_body`, `enter_block` and `dispatch_body`
  ([src/machine/execute/harness.rs](../../src/machine/execute/harness.rs)) — a
  `Vec<WorkingExpression>` and a `Vec<NodeId>` per block entry.

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

- *Buffers reachable only before the step brand opens — open.* `dep_sources` is built in
  `Host::step` before `sealed_continuation.open`, where `&mut self` is held, so the
  scratch handle is not reachable through `DecideCtx` there. Options: take the scratch
  borrow out of `self` before the `&mut` methods run, move the re-branding inside the
  open, or leave `dep_sources` on the heap. Recommended: move it inside the open, where
  the step's coverage is already in scope.
- *Block fan-out buffers — open.* `split_working_body` / `enter_block` / `dispatch_body`
  produce runs whose consumers sit on the harness side, past the decide's brand. Either
  they move with the fan-out or they stay heap-backed; decide once the arena's reach on
  the harness side is settled.

## Dependencies

**Requires:**

- [Step-scoped scratch arena](step-scratch-arena.md) — this item is the fan-out over that arena's remaining consumers.

**Unblocks:** none.
