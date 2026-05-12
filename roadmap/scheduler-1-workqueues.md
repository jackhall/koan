# Scheduler refactor phase 1 — Extract `WorkQueues`

**Problem.** [`Scheduler<'a>`](../src/runtime/machine/execute/scheduler.rs)
exposes its two routing queues as raw fields. Every push and pop site
re-states the routing rule (top-level submissions go to `queue`; internal
slots, woken consumers, and Replace-arm re-enqueues go to `ready_set`) and
the priority rule (`ready_set` drains ahead of `queue`) by hand. The five
distinct mutation sites are scattered across
[`submit.rs:93`](../src/runtime/machine/execute/scheduler/submit.rs#L93)
(top-level push), [`submit.rs:95`](../src/runtime/machine/execute/scheduler/submit.rs#L95)
(internal push), [`execute.rs:21-26`](../src/runtime/machine/execute/scheduler/execute.rs#L21-L26)
(prioritized drain), [`execute.rs:129`](../src/runtime/machine/execute/scheduler/execute.rs#L129)
(Replace-arm front push), and [`execute.rs:148`](../src/runtime/machine/execute/scheduler/execute.rs#L148)
(woken consumer push inside `notify_consumers`). Nothing in the type system
prevents a future caller from pushing a top-level submission onto
`ready_set` or a woken consumer onto `queue`.

**Impact.**

- *Routing rule type-enforced.* Each of the four push variants becomes a
  named entry point (`push_top_level`, `push_internal`,
  `push_internal_front`, `push_woken`); the call site's choice of method
  documents which routing arm it intends.
- *Priority rule type-enforced.* A single `pop_next` entry point drains
  `ready_set` ahead of `queue`; no caller can pop from one without
  observing the other.
- *Substrate for the rest of the refactor.* `WorkQueues` is independent of
  `DepGraph` and `NodeStore`, so landing it first puts the wrapper-shape
  pattern in place before the higher-risk extractions touch the
  bookkeeping fields.

**Directions.**

- *Sub-struct introduction — decided.* Add `WorkQueues` as a new sibling
  module under
  [`src/runtime/machine/execute/scheduler/`](../src/runtime/machine/execute/scheduler/).
  Two private fields (`queue: VecDeque<usize>`, `ready_set: VecDeque<usize>`);
  the only mutation paths are the five wrapper methods. `Scheduler<'a>`'s
  `queue` and `ready_set` fields are replaced by a single
  `queues: WorkQueues` field in the same edit.
- *Wrapper surface — decided.*
  `pop_next() -> Option<usize>`, `push_top_level(idx)`,
  `push_internal(idx)`, `push_internal_front(idx)`, `push_woken(idx)`. No
  other accessors — the queues are append-only-or-drain-only from outside.
- *Call-site migration — decided.* Every direct `self.queue.*` /
  `self.ready_set.*` reference in `submit.rs` and `execute.rs` migrates in
  the same commit that introduces the wrapper. `notify_consumers`'s woken
  push moves to `queues.push_woken(consumer)`; the `add` routing branch in
  `submit.rs` becomes `if … { self.queues.push_top_level(idx) } else {
  self.queues.push_internal(idx) }`; the Replace-arm push becomes
  `self.queues.push_internal_front(idx)`; the drain at the top of
  `execute` becomes `let idx = match self.queues.pop_next() { … }`.
- *No shim needed.* Since every push and pop site is converted in the same
  commit, no intermediate state exists where one wrapper exists alongside
  a raw field access. `cargo build` is green between phases.
- *Verification — decided.* `cargo build`, `cargo test`, and `cargo clippy
  --all-targets` all pass. Tests in
  [`scheduler/tests.rs`](../src/runtime/machine/execute/scheduler/tests.rs)
  and
  [`run/tests.rs`](../src/runtime/machine/execute/run/tests.rs)
  consume `Scheduler` through the public `add_dispatch` / `execute` /
  `read_result` surface and continue to pass unchanged.

## Dependencies

**Requires:**

**Unblocks:**
- [Scheduler refactor phase 2 — Extract `DepGraph`](scheduler-2-depgraph.md) —
  phase 2's call-site migration of the raw `dep_edges[idx].push(...)`
  sites in [`run_dispatch`](../src/runtime/machine/execute/run/dispatch.rs)
  and [`defer_to_lift`](../src/runtime/machine/execute/run.rs) lands on
  the substrate this phase establishes (wrapper-shape pattern already in
  place; no in-flight queue migration to interleave with).
- [Scheduler refactor phase 3 — Extract `NodeStore`](scheduler-3-nodestore.md) —
  `NodeStore::finalize`'s woken-consumer routing calls
  `WorkQueues::push_woken` directly, so the queue wrapper has to exist
  before `finalize` can take its final shape.
