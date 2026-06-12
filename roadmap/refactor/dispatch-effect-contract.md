# Pull the dispatcher out of the scheduler via a write-effect contract

Give the dispatcher the same treatment the **Invoke** (#24) and **Builtins** (#25)
rewrites gave `KFunction::invoke` and the builtin bodies: dispatch *decides* against a
read-only view of the scheduler and *returns* its scheduler mutations as an abstract
effect a harness interprets — so `Scheduler` becomes the **sole** `SchedulerHandle` impl
and dispatch stops holding `&mut Scheduler`.

**Problem.** [`DispatchCtx`](../../src/machine/execute/dispatch/ctx.rs) wraps
`&mut Scheduler<'run>` and is both the read surface and the write surface for every
dispatch shape handler. Unlike a builtin — which mutates its scope, returns one of four
[`Action`](../../src/machine/core/kfunction/action.rs)s, and never touches
`SchedulerHandle` — the shape handlers reach into the scheduler ~20 distinct ways,
interleaving reads and writes: dep-graph surgery (`add_park_edge`, `add_owned_edge`,
`clear_dep_edges`, `would_create_cycle`), slot reclaim (`free`), submission
(`add_dispatch_here`, `schedule_*_literal`), and live result reads (`is_result_ready`,
`read_result`). `DispatchCtx` additionally carries a full `SchedulerHandle` impl
(`ctx.rs:312`) solely so a builtin invoked *mid-dispatch* (e.g. `newtype_construct`) sees
the dispatcher's frame/chain. So `SchedulerHandle` keeps **two** impls and the dispatch
shape modules can't be unit-tested without a live scheduler.

The surface partitions cleanly once the writes are separated from the reads:

- **Static-over-the-dispatch reads → snapshot into an immutable context.** `current_scope`,
  `chain_deref`, `active_chain`, `current_lexical_chain`, `in_contract_chain`,
  `build_bare_outcomes` are fixed for the slot's lifetime — fields of a read-only `DispatchCx`,
  not method calls.
- **Live reads of *other* slots → a shared `&Scheduler`.** `is_result_ready` /
  `would_create_cycle` on **pre-existing** producers found by name resolution (the
  `Parked(p)` arms in [`keyworded.rs`](../../src/machine/execute/dispatch/keyworded.rs),
  the TypeCall head placeholder in
  [`single_poll.rs:417`](../../src/machine/execute/dispatch/single_poll.rs)) read graph state
  dispatch never wrote — no interleave hazard.
- **Writes → a returned effect.** `add_park_edge` / `add_owned_edge` / `clear_dep_edges` /
  `free` are pure side effects with no in-step read-back; they defer into a plan a harness
  applies. (`replace_with_parked_dispatch` / `defer_to_lift` aren't writes — they build the
  terminal `NodeStep`, already the step's return value.)

Two clusters look like they resist the split but don't:

- *Cycle-check → edge-add is already phase-separated.* `keyworded.rs:302–314` runs all
  `would_create_cycle` checks into a `to_wait` list, then adds the park edges in a separate
  loop — every check reads the graph as of step entry, so the edge-writes defer with no
  reordering hazard. The other `would_create_cycle` sites (`keyworded.rs:420,461`) gate a
  `SchedulerDeadlock` *error*, not an edge.
- *The eager-subs inline splice never reads back a fresh submission.* `install_eager_subs`
  (`ctx.rs`) appears to submit-then-read-then-splice per sub, but submission is
  enqueue-then-drain (`submit_node`, `scheduler/submit.rs` — nothing executes during
  submit), so a freshly minted sub is **never** `is_result_ready` in the same step. The
  eager-splice branch only ever fires for `PendingSub::Reuse(id)` — an *already-resolved
  pre-existing* binder pre-sub (`keyworded.rs:407`). Readiness is therefore a read of a
  static-over-this-step slot, knowable *before* any submission. A `debug_assert!` at the
  submission site now locks this invariant (verified green across the full test slate).

**Acceptance criteria.**

- `Scheduler` is the only `impl SchedulerHandle`; the `DispatchCtx` impl
  (`ctx.rs:312–429`) is gone. Dispatch holds `&Scheduler` (reads) + an immutable
  snapshot context, never `&mut Scheduler`.
- Each dispatch shape handler is split into a decision step (returns a dispatch effect)
  and a harness that applies the writes — the peer of `run_action` / `harness.rs` for
  builtins. The write half of the old `DispatchCtx` surface (`add_park_edge`,
  `add_owned_edge`, `clear_dep_edges`, `free`, submission) lives only in the harness.
- **Eager-subs is modelled as dispatch's own `Combine`.** A handler declares its deps
  (fresh exprs to dispatch + existing producers to park on) and a *finish that splices the
  resolved values into `working_expr`*; the splice — writing `ExpressionPart::Future` cells
  into a `KExpression` — lives entirely in the finish. The scheduler learns nothing about
  `Future` cells or splicing; it resolves deps and hands values back exactly as it does for
  a builtin `Action::Combine`. `EagerSubsInstall` / `install_eager_subs` /
  `resume_eager_subs` collapse into that shape.
- A builtin invoked mid-dispatch (`newtype_construct`) routes through the shared action
  harness with the dispatcher's ambient frame/chain supplied as input — not via a
  `SchedulerHandle` impl on the dispatch context.
- The heavier dispatch state structs (`KeywordedState`, `FnValueState`, the eager-subs
  track) survive as carriers of the Combine finish/continuation — they do **not** collapse
  into `'run` closures (the builtins' `Cont` pattern) — but their "effect" is a
  Combine-shaped dep declaration, scheduler-unaware.
- The `debug_assert!` locking "a freshly-submitted sub is never immediately ready" stays.

**Directions.**

- *Read/write split over full decide/do — decided.* Dispatch keeps live reads of
  pre-existing slots behind `&Scheduler`; only the writes become an effect. Full
  scheduler-unawareness (the builtin model) is not a goal — dispatch genuinely reads
  evolving graph state.
- *Eager-subs as a Combine, not a submit-and-splice primitive — decided.* Splice stays in
  dispatch; the scheduler never gains splice knowledge. This is strictly cleaner than the
  two rejected forks (a harness "submit-and-splice" primitive leaks `Future` knowledge into
  the harness; a narrow `&mut Submitter` capability keeps `&mut` in dispatch).
- *Keep explicit state structs, not continuation closures — decided.* The dispatch states
  carry more than a builtin's one-shot `Cont` and would fight the borrow checker as
  `'run` closures.
- *Shape of the dispatch effect enum — open.* The write vocabulary (park-edge / owned-edge
  / clear / free / terminal `NodeStep`) plus the Combine-dep declaration must reduce to a
  closed set the harness interprets. The inventory of every shape handler's writes is the
  first concrete task; whether park/owned/clear/free become distinct effect variants or one
  `Vec<GraphWrite>` plan rides alongside the `NodeStep` is the open design fork.
- *Where the ambient frame/chain for mid-dispatch builtin invokes is threaded — open.*
  Today `with_active_frame` re-hands `&mut DispatchCtx` into the builtin; after the split
  the dispatcher's frame/chain must reach `run_action` as input without a `SchedulerHandle`
  forward.

## Dependencies

The capstone of the scheduler-extraction arc begun by the Invoke (#24) and Builtins (#25)
rewrites. Touches the execution model
([design/execution-model.md](../../design/execution-model.md)) — update the dispatch /
`SchedulerHandle` description there when the second impl is removed.

**Requires:** none beyond the shipped Invoke/Builtins `Action` harness it mirrors.

**Unblocks:** retiring `SchedulerHandle` as a trait once `Scheduler` is its only impl (the
trait can become inherent methods, or the read/write surfaces can split into two narrower
traits).
