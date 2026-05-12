# Scheduler refactor phase 3 — Extract `NodeStore`

**Problem.** The slot table on
[`Scheduler<'a>`](../src/runtime/machine/execute/scheduler.rs#L30-L62) is
three vectors that share an index space (`nodes`, `results`, `free_list`)
plus the lifecycle that moves a slot through them: `alloc_slot →
take_for_run → reinstall* → finalize → free_one`. Every transition is
open-coded:

- *Slot allocation* in
  [`add`](../src/runtime/machine/execute/scheduler/submit.rs#L62-L80)
  pops `free_list` (six lines reset) or extends all three vectors in
  parallel (six lines push); nothing in the type system prevents a future
  call site from growing one vector without the others.
- *The take/reinstall pair* at
  [`execute.rs:28-30`](../src/runtime/machine/execute/scheduler/execute.rs#L28-L30)
  and the Replace arm at
  [`execute.rs:121-126`](../src/runtime/machine/execute/scheduler/execute.rs#L121-L126)
  bracket every slot run; the intermediate state where `nodes[idx]` is
  `None` is observable from anywhere in the file.
- *Terminal write + notify-walk* fires from four sites in `execute.rs`
  ([`74-75`](../src/runtime/machine/execute/scheduler/execute.rs#L74-L75),
  [`80-81`](../src/runtime/machine/execute/scheduler/execute.rs#L80-L81),
  [`93-94`](../src/runtime/machine/execute/scheduler/execute.rs#L93-L94),
  [`97-98`](../src/runtime/machine/execute/scheduler/execute.rs#L97-L98)).
  Every site is `self.results[idx] = Some(...)` immediately followed by
  `self.notify_consumers(idx)`; the "every terminal write fires the
  notify-walk" rule survives as a convention nothing enforces.
- *Reclamation* in
  [`free`](../src/runtime/machine/execute/scheduler/execute.rs#L164-L180)
  clears `nodes[i]` (implicitly via the early-continue guard),
  `results[i]`, and pushes onto `free_list` in three separate statements;
  nothing prevents a recycler from clearing two of three.

**Impact.**

- *Index-space invariant type-enforced.* `alloc_slot(node, owned_edges) ->
  usize` is the only path that picks an index; the `nodes` / `results` /
  `free_list` triple update happens inside one method body. The
  recycle-vs-extend choice is made in one place.
- *Run-window invariant type-enforced.* `take_for_run(idx) -> Node<'a>`
  and `reinstall(idx, node)` bracket every slot run. Every `take` is
  matched with either a later `reinstall`, a `finalize`, or a `free_one`;
  the intermediate `nodes[idx] == None` state is internal to `NodeStore`.
- *Terminal-write invariant type-enforced.* `finalize(idx, output)` is the
  only path that writes a terminal `NodeOutput` into `results[idx]`.
  Single method takes `NodeOutput` directly (no `finalize_value` /
  `finalize_err` split — verified against the four call sites, which all
  already construct a unified `NodeOutput` before the write). The
  cross-struct `Scheduler::finalize` is `let woken =
  self.store.finalize(idx, output); for c in woken {
  self.queues.push_woken(c) }`, where `woken` is `DepGraph::drain_notify`'s
  return value chained through. `notify_consumers` collapses entirely
  into this method — verified: every existing call site is preceded by a
  terminal `results[idx]` write.
- *Reclaim invariant type-enforced.* `free_one(idx) -> Vec<DepEdge>` is
  the only path that clears `nodes[idx]` and `results[idx]`; it pushes
  onto `free_list` and returns the slot's edges. The cascade-free walk in
  `Scheduler::free` consumes the returned edges and pushes their owned
  children onto its iterative stack — one operation clears the whole
  slot record.
- *Read-side surface relocates.* `is_result_ready`, `read_result`, `read`,
  `len`, `is_empty` move from `Scheduler` to `NodeStore`. They project
  `results` and `nodes` and don't touch the other sub-structs;
  `Scheduler`'s public surface (`read_result`, `read`, `len`, `is_empty`)
  re-exposes them by delegation.

**Directions.**

- *Sub-struct introduction — decided.* Add `NodeStore` as a sibling module
  under
  [`src/runtime/machine/execute/scheduler/`](../src/runtime/machine/execute/scheduler/).
  Three private fields (`nodes`, `results`, `free_list`); the only
  mutation paths are the wrapper methods listed below.
  `Scheduler<'a>`'s three slot-table fields are replaced by a single
  `store: NodeStore<'a>` field in the same edit, alongside the surviving
  `active_frame` (stays on `Scheduler` — its save/restore is local to one
  ~5-line scope in `execute.rs` and doesn't earn a wrapper).
- *Wrapper surface — decided.*
  `alloc_slot(node, owned_edges) -> usize`,
  `take_for_run(idx) -> Node<'a>`,
  `reinstall(idx, node)`,
  `finalize(idx, output: NodeOutput<'a>) -> ()` (with
  `Scheduler::finalize` calling `deps.drain_notify(idx)` for the woken
  set in the same orchestrating method),
  `free_one(idx) -> Vec<DepEdge>`,
  plus the read-side accessors `is_result_ready`, `read_result`, `read`,
  `len`, `is_empty`.
- *`notify_consumers` absorption — decided.* Verified against the four
  call sites in
  [`execute.rs`](../src/runtime/machine/execute/scheduler/execute.rs):
  every `self.notify_consumers(idx)` is immediately preceded by a
  terminal `self.results[idx] = Some(...)`. `notify_consumers` does not
  survive as a private helper; its body collapses into
  `Scheduler::finalize`, which is the single method the four call sites
  invoke instead. Single `finalize(NodeOutput)` shape — no value/err
  split.
- *Cross-struct composition — decided.* Three methods stay on `Scheduler`
  and orchestrate calls into the sub-structs:
  - `Scheduler::add` calls `store.alloc_slot(...)`, then
    `deps.reset_slot_deps(idx, owned_edges)` for the recycle case (or
    nothing extra for the extend case, since `alloc_slot` extends both
    halves of the index space and `extend_for_new_slot` on `DepGraph`
    extends the dep half), then `deps.register_slot_deps(idx)`, then
    routes to `queues`.
  - `Scheduler::finalize(idx, output)` calls `store.finalize(idx,
    output)`, then `for c in self.deps.drain_notify(idx) {
    self.queues.push_woken(c) }`. The four `execute.rs` call sites
    collapse to a single `self.finalize(idx, output);` line each.
  - `Scheduler::free(idx)` runs an iterative stack-walk calling
    `store.free_one(i)`, pushing the returned `DepEdge::Owned` children
    onto the stack (and ignoring `DepEdge::Notify` per the cascade-walk
    invariant). The current `execute.rs:164-180` body collapses
    accordingly.
- *`active_frame` — decided.* Stays as a `Scheduler` field.
  The save/restore pattern around each slot run is local to one ~5-line
  scope in
  [`execute.rs:37-45`](../src/runtime/machine/execute/scheduler/execute.rs#L37-L45)
  and doesn't earn a wrapper. `SchedulerHandle::current_frame` continues
  to read `self.active_frame` directly.
- *Call-site migration — decided.* Every direct `self.nodes[idx]` /
  `self.results[idx]` / `self.free_list` reference across
  `submit.rs` (the recycle/extend arms of `add`), `execute.rs` (the
  drain's `take`, the Replace arm's `reinstall`, the four terminal-write
  sites, the `free` body), `run/finish.rs` (the `read_result` in
  `run_bind` / `run_combine` / `run_lift`), and `run/dispatch.rs`
  (`is_result_ready` in the replay-park check) migrates in the same
  commit that introduces `NodeStore`. No half-migrated intermediate.
- *Verification — decided.* `cargo build`, `cargo test`, and `cargo clippy
  --all-targets` all pass. Existing tests in
  [`scheduler/tests.rs`](../src/runtime/machine/execute/scheduler/tests.rs)
  and
  [`run/tests.rs`](../src/runtime/machine/execute/run/tests.rs)
  exercise the lifecycle end-to-end. Optionally run
  [`cargo +nightly miri test`](../TEST.md#miri-audit-slate) on the audit
  slate — the slot lifecycle invariants this work codifies are the same
  ones the slate's leak and aliasing tests already exercise. Run
  `tools/modgraph.py` to confirm `Scheduler`'s field count is now 4
  (`deps`, `queues`, `store`, `active_frame`) and the three new internal
  modules each have a single tight invariant.

## Dependencies

**Requires:**
- [Scheduler refactor phase 2 — Extract `DepGraph`](scheduler-2-depgraph.md) —
  `NodeStore::finalize`'s natural shape composes with
  `DepGraph::drain_notify`. `Scheduler::finalize`'s orchestrating body
  is `let woken = self.store.finalize(idx, output); for c in
  self.deps.drain_notify(idx) { self.queues.push_woken(c) }` — designing
  `NodeStore::finalize` before `DepGraph`'s API is fixed risks churn.

**Unblocks:** none on the language roadmap. The partitioning this
phase lands is plausibly useful substrate for the JIT-snapshot
question contemplated in the static-typing-and-jit item, but that
dependency is soft (the snapshot question is itself open) and not
listed as a structured edge.
