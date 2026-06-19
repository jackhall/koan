# The scheduler runtime

How dispatch is modeled as scheduler work and how the DAG runs it: the
decide→outcome→apply contract at the dispatcher/scheduler boundary, the `Outcome`
return shapes, push/notify dependency edges and their invariants, the bare-name
and working-copy splices, tail-call rewriting, transient-node reclamation, and the
one engine that serves both build-time and run-time execution. Part of the
[execution model](README.md).

## Dispatch as a scheduler node

The scheduler models dispatch itself as a node. There is one node shape — a
[`NodeWork`](../../src/machine/execute/nodes.rs) struct that waits on a set of deps
and then runs a [`NodeCont`](../../src/machine/execute/outcome.rs) closure over
their resolved terminals. A top-level expression enters as a *dispatch decide*: a
`NodeWork` whose `cont` classifies the expression on first poll
([`schedule_expr`](../../src/machine/execute/runtime/interpret.rs) collapses to "add one
dispatch decide per top-level expression"; the rest is dynamic). At run time a
decide walks its expression's parts, spawns sub-dispatch nodes for nested
sub-expressions, and a builtin body can declare further dispatch nodes as deps of
the `Outcome` it returns.

Per-family behavior — dep-finish vs. catch vs. decide — is not a node variant; it is
which combinator built the `cont` closure ([`short_circuit`](../../src/machine/execute/outcome.rs)
/ [`catch_cont`](../../src/machine/execute/outcome.rs) /
[`ignore_results`](../../src/machine/execute/outcome.rs) in
[`outcome.rs`](../../src/machine/execute/outcome.rs)). The node itself never branches
and names no AST.

- A **dep-finish** `cont` (built by `short_circuit`) waits on a fixed set of dep
  slots, short-circuits on the first errored dep, and otherwise runs an arbitrary
  host closure ([`DepFinish`](../../src/machine/execute/outcome.rs)) over their
  resolved values. List- and dict-literal planners use it; the construction logic
  — including already-resolved literal scalars that don't need a dep slot — lives
  in the closure's capture.
- A **catch** `cont` (built by `catch_cont`) waits on one slot and hands its
  terminal to a [`CatchFinish`](../../src/machine/execute/outcome.rs) closure as a
  `Result<&KObject, KError>`. Unlike a dep-finish, an errored dep does not
  short-circuit — the closure always runs and decides whether to recover or
  re-raise. The `TRY-WITH` builtin
  ([`try_with`](../../src/builtins/try_with.rs); see
  [error-handling.md](../error-handling.md)) is the sole caller today: it spawns its
  watched expression as a sub-dispatch and registers a catch that picks the
  matching branch by tag.
- A **decide** `cont` (built by `ignore_results`) takes no dep values — it reads
  the view and classifies / re-resolves — so its deps are park-only and the
  results slice is ignored.

## The dispatcher / scheduler boundary

The scheduler ([`scheduler`](../../src/scheduler.rs)) is a crate-root sibling of
`machine`, not nested inside it: a workload-independent DAG of dependency-linked
nodes, generic over a [`Workload`](../../src/scheduler/workload.rs) and naming no Koan
value, error, scope, memory, or AST type. The Koan interpreter is the sole
workload — `machine::execute` instantiates it as `Scheduler<KoanWorkload>` and
drives it from the run loop
([`execute/run_loop.rs`](../../src/machine/execute/run_loop.rs)) through the
scheduler's inherent-method contract.

The dispatch tree
([`execute/dispatch/`](../../src/machine/execute/dispatch.rs)) and the run-loop driver
are both Koan-side. Every scheduler-facing step — a dispatch decide, a finish, a
builtin body, an invoke — flows through one **decide → outcome → apply** contract:
it decides against a read-only view, *returns* the scheduler mutations it wants as
data, and a single harness method applies them. The three pieces:

- **The read view** —
  [`SchedulerView<'step, 'view>`](../../src/machine/execute/dispatch/ctx.rs) wraps
  `&'view Scheduler<KoanWorkload>` (never `&mut`) together with the driver's per-step
  ambient context. It exposes only the reads a decide needs: the
  static-over-the-step ones (`current_scope`, `chain_deref`, `in_contract_chain`,
  `build_bare_outcomes`) and the live reads of *pre-existing* producers
  (`is_result_ready`, `would_create_cycle`, `read_result`). It permits scope
  binding (interior-mutable `&Scope`) but no graph write. The scheduler's `queues`
  / `deps` / `store` fields stay `pub(in crate::scheduler)`; the dispatch shape
  modules (`keyworded`, `fn_value`, `single_poll`) never name scheduler fields
  directly.
- **The effect** —
  [`Outcome<'step>`](../../src/machine/execute/outcome.rs) is the one currency
  every producer and finish returns (the dispatch-side peer of the builtin
  [`Action`](../../src/machine/core/kfunction/action.rs)). It is AST-free — no
  variant names a `KFunction` or a `KExpression`. Its single lifetime `'step` is the
  per-step cart-scale frame lifetime the `Done` value is born at; the consumer pull-lifts it
  across each dep edge ([per-call-region/lifecycle.md § Consumer-pull node-output lift](../per-call-region/lifecycle.md#consumer-pull-node-output-lift)).
  Four variants: `Done` (the node's terminal value at `'step`, or an
  error), `Continue` (replace this slot's work and frame,
  re-run, no park), `ParkThenContinue` (park on deps, then run a
  [`Continuation`](../../src/machine/execute/outcome.rs) that yields another
  outcome), and `Forward` (the slot's result *is* a named producer's — the
  harness splices the slot out as an alias of that producer rather than
  installing a forwarding node; see
  [Bare-name forward splice](#bare-name-forward-splice)). The dispatch→execution
  hand-off is itself a dep-free `Continue`: a decide that picks a call folds the
  resolved call into a `Continue` whose frame placement installs the per-call
  cart (a user fn's `ReuseReserve`, a builtin's `Inherit`) and whose `work`
  re-decides via the folded `invoke` / re-resolve closure on the next pop, so no
  variant carries the call's AST. Each is pure data — no `&mut Scheduler` is
  captured.
- **The write harness** —
  [`KoanRuntime<'run>`](../../src/machine/execute/runtime.rs) owns the `Scheduler`
  by composition (a `sched` field, not a `&mut` borrow) and is the **sole**
  holder of `&mut Scheduler` across the execute tree. The per-step *ambient*
  state — the active per-call frame, the slot reserve, the run frame, the
  executing slot's opaque payload, and the contract-chain flag — lives on the
  driver ([`ambient`](../../src/machine/execute/ambient.rs)), not the scheduler,
  which is a pure DAG runtime. Its
  [`apply_outcome`](../../src/machine/execute/runtime.rs) interprets a returned
  outcome into graph writes and the slot's `NodeStep`. Because only the harness
  reborrows the scheduler mutably, no decide handler holds `&mut Scheduler` —
  decide (against a read-only view) and apply (against `&mut self`) never
  overlap, and that separation is structurally enforced by the type rather than
  a naming convention. The execute loop, the AST-aware submission wrappers
  (`enter_block`, `dispatch_in_own_scope`, `dispatch_in_active_frame`,
  `dispatch_body`, `submit_dep_finish_in_own_scope`), `submit_dispatch`, and the
  aggregate-literal lowering are all `&mut self` methods on `KoanRuntime`. The
  unified node handler
  ([`run_step`](../../src/machine/execute/run_loop.rs)) collects the slot's
  resolved dep terminals, builds a `SchedulerView`, runs the `cont` closure,
  reclaims the owned-dep suffix, and hands the outcome to `apply_outcome`.

The scheduler reaches the driver only through its method contract, and every
method names only `NodeId` and the workload's associated types — no signature
names a `KExpression`, `Scope`, or AST type. `pop_next` / `take_for_run` /
`replace` drive a slot's lifecycle; `submit_node` and the alias-resolving edge
installs (`add_owned_edge` / `add_park_edge` / `splice_forward`) wire the graph;
`finalize` / `free` / `reclaim_deps` terminalize and reclaim; `read*` /
`is_result_ready` / `would_create_cycle` / `unresolved` are the reads. No trait
wraps `Scheduler`: those are inherent methods capped `pub(crate)`, so only the
Koan driver reaches them, and the `queues` / `deps` / `store` fields stay
`pub(in crate::scheduler)`. A builtin invoked mid-dispatch
(e.g. `newtype_construct`) routes through the shared
[`run_action`](../../src/machine/execute/runtime.rs) harness as a pure
`Action → Outcome` lowering; `exec::invoke` reads the dispatcher's ambient
`current_frame` / `current_lexical_chain` off the view to build the builtin's
`BodyCtx`.

## Callable result — the `Outcome` return shapes

A builtin or user-fn body, like every other step, returns an
[`Outcome`](../../src/machine/execute/outcome.rs):

- `Done(Value)` — the body produced a final value; the slot finalizes.
- `Done(Err)` — structured failure; see [error-handling.md](../error-handling.md).
- `Continue` — the body wants to dispatch a fresh expression in its own slot
  (TCO, see below); when the body has leading (non-tail) statements they
  become owned deps the slot parks on, and the `Continue` fires only from the
  resolving finish.

When a body cannot produce its result inline — its expression has nested
sub-expressions whose own evaluation hasn't run yet — the slot parks: its work is
rewritten to a `NodeWork` that waits on the spawned sub-dispatch deps and runs a
dep-finish `cont` that assembles the result on wake. The slot keeps its index, so
consumers downstream see the eventual terminal under the original slot index as
if the body had produced it directly.

A bare-name slot whose result *is* a single producer's result is a special case:
rather than park as a forwarding node, it is spliced out as an alias of that
producer (see [Bare-name forward splice](#bare-name-forward-splice)), keeping the
single-producer-per-result invariant without a duplicate slot.

## Push/notify dependency edges

The scheduler's edges point producer → consumer. Each slot's `DepRow` carries a
`notify: Vec<NodeId>` list of dependents waiting on it; each consumer carries a
`pending: usize` counter of unresolved deps. When a slot writes a terminal
`Value` or `Err`, the notify-walk drains its `notify` list, decrements each
consumer's `pending`, and pushes any zero-counter consumer onto the run-set.
The terminal write and notify-walk fire in a single
[`Scheduler::finalize`](../../src/machine/execute/run_loop.rs)
method body that pairs `NodeStore::finalize` with `DepGraph::drain_notify`,
so the "every terminal write fires the notify" rule is type-enforced
rather than restated at each call site. Consumers arrive on the run-set
only when actually ready; there is no poll-and-requeue.

Every consumer wakes the same way: at pop time its `pending_deps` is zero, so
every dep is terminal, and [`run_step`](../../src/machine/execute/run_loop.rs)
reads each resolved dep off the view by index and hands the `Result` slice to the
slot's `cont`. There is no per-edge wake-attribution side-channel — a decide that
re-resolves reads its producers from the rebuilt scope, not a wakes list.
`DepGraph::drain_notify` returns the per-consumer `hit_zero` flag so the
enqueue-on-zero runs off a single drain.

The run-set has two priority bands managed by
[`WorkQueues`](../../src/scheduler/work_queues.rs). Internal
work — notify-walk wake-ups, Replace-arm re-enqueues, and ready-on-arrival
nodes registered in `add()` — routes through `WorkQueues::push_internal` /
`push_internal_front` / `push_woken`. Top-level `dispatch_in_scope` calls route
through `WorkQueues::push_top_level` so independent top-level expressions
execute in submission order. The execute loop drains via `WorkQueues::pop_next`,
which yields internal slots ahead of top-level slots; the routing rule (which
band a push lands in) and the priority rule (which band a pop drains first)
are both enforced by the wrapper's method surface rather than restated at each
call site.

## Dependency graph invariants

[`DepGraph`](../../src/scheduler/dep_graph.rs) stores one
`rows: Vec<DepRow>` parallel to the slot table; each `DepRow` bundles the
three coordinated per-slot fields — `notify` (forward wake edges to this
slot's dependents), `pending` (this slot's unresolved-dep counter), and
`edges` (backward edges to producers it depends on, tagged `Owned` or
`Notify`) — and the rows uphold three invariants:

- **Inv-A (wake-pending coherence).** For every consumer slot `c`,
  `rows[c].pending == |{ p : c appears in rows[p].notify }|`. Mutations go
  through the row, so a slot's `notify` / `pending` / `edges` cannot
  desync — Inv-A holds by construction.
- **Inv-B (free-cascade source).** `rows[c].edges` lists every `Owned`
  sub-slot `c` must cascade-reclaim. Park edges are tagged `Notify` and
  filtered out of `free`'s walk via `owned_children`. Independent of
  Inv-A.
- **Inv-C (lazy notify-scrub on free).** A slot `c` is only freed once
  every producer's `drain_notify` has run and removed `c` from every
  `rows[*].notify`. The
  `freed_slot_does_not_appear_in_other_notify_lists` test pins this;
  `free` relies on Inv-A and Inv-C still holding rather than scrubbing
  itself.

Inv-B is what makes the eager `clear_dep_edges(idx)` in
`Scheduler::reclaim_deps` sound at the `[park_count..]` owned-suffix reclaim: the
suffix a node owns holds only `Owned` edges (the sub-Dispatches the slot spawned).
`Notify` edges land only in the `[..park_count]` prefix — a dispatch decide's
park-on-producer and a dep-finish's `Existing` sibling parks — which `run_step`
excludes from the reclaim by reading `deps[park_count..]` for the owned indices,
so clearing the owned tree cannot drop a wake intent on a sibling producer.

## Bare-name forward splice

The push/notify model assumes a single producer slot per result. A bare-name slot
(`(some_var)`, or the RHS of `LET y = z`) that resolves its name to a still-running
binding-producer would otherwise become a *second* producer of that result. Instead
the slot is **spliced out** as an alias of the producer, which stays the sole
producer. All the graph logic lives in
[`scheduler/splice.rs`](../../src/scheduler/splice.rs):

- The bare-name decide returns [`Outcome::Forward(producer)`](../../src/machine/execute/outcome.rs).
  If `producer` is already ready, the harness finalizes the slot with the
  producer's terminal directly ([`NodeStep::Done`](../../src/machine/execute/nodes.rs)).
- Otherwise the slot's step yields [`NodeStep::Alias(producer)`](../../src/machine/execute/nodes.rs),
  and the execute loop calls [`Scheduler::splice_forward`](../../src/scheduler/splice.rs):
  the consumers already parked on the slot are moved onto the producer's notify
  list ([`DepGraph::splice_notify`](../../src/scheduler/dep_graph.rs)),
  and the slot's [`SlotState`](../../src/scheduler/node_store.rs)
  becomes `Aliased(producer)`. The aliased slot never fires; the producer's fire
  wakes the moved consumers directly.

Reads follow the alias to the real producer:
[`Scheduler::resolve_alias`](../../src/scheduler/splice.rs) walks the
alias chain (iterative, always pointing downstream to a real producer, so it
terminates and never cycles), and `read_result` / `is_result_ready` resolve
through it. Edge installs resolve it too:
[`add_owned_edge`](../../src/scheduler/splice.rs) /
[`add_park_edge`](../../src/scheduler/splice.rs) wire a late consumer
to the *resolved* producer, and a producer that has already finalized adds no edge
at all — the consumer reads its value directly when it runs, contributing nothing
to its pending count. So neither the store nor the dep graph has to be
alias-aware on its own; the alias contract lives in one module.

## Working-copy splice

The scheduler dispatches each expression by mutating an **owned working
copy** of it. The keyworded dispatcher extracts every nested sub-expression out of
the parent's `parts` (replacing each with a placeholder `Identifier`) and
declares them as the deps of a
[`ParkThenContinue`](#the-dispatcher--scheduler-boundary) whose continuation
is a `Continuation::Finish` — the dispatch flavor of a dep-finish. The harness
submits each dep as a sub-Dispatch and parks the parent on a
[`NodeWork`](../../src/machine/execute/nodes.rs) whose `cont` is a dep-finish wrapping
that *splice finish* (a [`DepFinish`](../../src/machine/execute/outcome.rs)
closure). When the deps terminalize, that finish runs and writes each
resolved value back into the working copy:
`working_expr.parts[part_idx] = ExpressionPart::Future(value)`. The splice
lives **entirely inside the finish** — the scheduler resolves deps and hands
values back exactly as it does for any dep-finish, learning nothing about `Future`
cells. The assembled `Future`-laden expression then goes through
`resolve_dispatch` as if it had been written with literals.

(This *expression* splice — rewriting `parts` to `Future` cells — is distinct
from the *slot* splice of [Bare-name forward splice](#bare-name-forward-splice),
which aliases one slot to another. They share the word but not the mechanism.)

Source-of-truth ASTs are never mutated. The working copy is cloned from
its source at slot-submission time — the user-fn body executor clones each
body statement onto its slot, `match_case::body` and `try_with` clone their picked arm, top-level
expressions move into the slot at `dispatch_in_scope`. The splice mutates the
slot-owned copy and nothing else; the next call to the same FN clones the
body fresh.

The splice gives typed-slot dispatch a uniform input shape: sub-Dispatch
results land in the same positions as literals would, so the
slot-specificity scoring path is unified across builtins, user-fns, and
pre-evaluated sub-expressions. The cost — body clone per call, one slot
per nested `(...)` — and what it buys are detailed in
[Performance characteristics](calls-and-values.md#performance-characteristics).

## Tail-call optimization

An [`Action::Tail`](../../src/machine/core/kfunction/action.rs), lowered to an
[`Outcome::Continue`](../../src/machine/execute/outcome.rs) by `run_action`,
makes a tail return rewrite the **current scheduler slot's work** to a fresh
dispatch decide of `expr` and re-run in place — no new node allocated. Both deferring
builtins (`match_case`, and `run_user_fn` for user-fns) are tail by
construction. A chain of tail calls (`A → B → PRINT`, or unbounded
`LOOP → LOOP`) reuses one slot end-to-end. Verified by two slot-count
assertions in the test suite. When a body has leading (non-tail) statements,
they become owned deps the slot parks on (one body-block `DepRequest::BodyBlock`) and
the `Continue` tail fires only from the resolving finish — so the leading
siblings run, and cascade-free, before the tail-replace, restoring frame
uniqueness so [`try_reset_for_tail`](../per-call-region/frames.md#tco-frame-reuse)
reuses the cart and TCO stays flat even for side-effecting multi-statement
bodies.

The slot's `Rc<CallFrame>` is held in exactly one place during each step,
which is what lets the tail-reuse path detect "nothing escaped" and reset
the frame shell in place across iterations rather than allocating a fresh
one. See
[per-call-region/frames.md § TCO frame reuse](../per-call-region/frames.md#tco-frame-reuse).

A subtle point: host-stack overflow on naïve recursion is solved by the graph
model itself, not by `Tail`. Every "recursive call" enters the scheduler's
run-set rather than growing the Rust call stack — that property is
structural, not optimizing. What `Tail` adds is constant **scheduler-vec**
memory across the tail-call chain; frame reuse on top of it keeps **heap
memory** constant too.

## Transient-node reclamation

`Tail` reuses the outermost slot but bodies typically have internal
sub-expressions — the predicate of an `IF`/`MATCH` guard, the argument
expressions of a recursive call, list/dict literal elements. Each spawns
a sub-Dispatch that the parent slot parks on as an owned dep. Without
reclamation those slots accumulate per body iteration, so realistic recursive
code is O(n) scheduler memory even when its data footprint is O(1).

Reclamation runs in [`run_step`](../../src/machine/execute/run_loop.rs) after
the `cont` closure returns its `Outcome`, before the harness applies it — so a
dispatch splice finish's freed indices are on the free-list before the harness
dispatches the spliced body. Once the consumer has read its dep results and either
spliced them into `working_expr.parts` as `Future(value)` (the eager-subs splice
finish) or handed them to its dep-finish / catch finish, the owned dep
slots are unreachable: a sub-Dispatch is
owned by exactly one consumer, recorded in the consumer's `dep_edges`
entry as a `DepEdge::Owned(NodeId)`. Free walks recursively, recycling
each dep's own dep tree, and stops at any still-live slot via
`NodeStore::is_live` — so a free that dives into another in-flight
user-fn call leaves that subtree for that call's own reclamation.

The net effect: recursive bodies whose only persistent state is the call
result run in O(1) scheduler memory across iterations, with the per-iteration
fanout (the body's transient sub-Dispatches) recycled through a
free-list of slot indices that `add()` pulls from before extending the vecs.
Slot-table state lives in a
[`NodeStore`](../../src/scheduler/node_store.rs)
sub-struct on `Scheduler` that owns a single `slots` vector of `SlotState`
enums plus a `free_list: Vec<NodeId>` of recyclable indices. One enum encodes
the per-slot lifecycle — `PreRun(Node)` (an un-run node payload), `Running`
(payload moved out for its step), `Done(Result)` (terminal result),
`Aliased(NodeId)` (a bare-name forward spliced out to its producer), and `Free`
(reclaimed) — and each index moves through `alloc_slot → take_for_run →
reinstall* → finalize → free_one`. Each transition is a single atomic mutator
body, so the recycle-vs-extend choice, the take/reinstall pairing, the terminal
write, and reclamation are each encapsulated; because payload and result are the
same enum slot, no call site outside `NodeStore` can land a `Done` without the
node having been taken, nor read a result before it is `Done`.
Dependency bookkeeping lives alongside it in a single
[`DepGraph`](../../src/scheduler/dep_graph.rs) sub-struct
that owns one `rows: Vec<DepRow>`, each `DepRow` bundling the three
coordinated per-slot fields — `notify: Vec<NodeId>` (this slot's dependent
list), `pending: usize` (its unresolved-dep counter), and `edges:
Vec<DepEdge>` (its backward edges to producers, tagged `Owned` or `Notify`;
the `Owned` arm carries the ownership tree the free walk follows, and the
`Notify` arm carries park-only edges that the walk skips). The rows are kept
private and mutated only through a small surface (`install_for_slot`,
`add_owned_edge`, `add_park_edge`, `drain_notify`, `owned_children`,
`clear_dep_edges`, `splice_notify`) so every change preserves the per-row
invariant atomically — every forward edge in `rows[p].notify` has a matching
backward entry in `rows[c].edges` and contributes 1 to `rows[c].pending`.
`add_owned_edge` / `add_park_edge` (in
[`splice.rs`](../../src/scheduler/splice.rs)) resolve the
producer through any alias and short-circuit a producer that is already
terminal; `splice_notify` moves a spliced-out slot's dependents onto its
producer's row.
`Scheduler::add` orchestrates across the two sub-structs: `NodeStore::alloc_slot`
picks the index (popping `free_list` or extending) and `DepGraph::install_for_slot`
branches privately on whether the slot is recycled or freshly extended to
write the dep entries in lockstep. See also
[memory-model.md § Performance notes](../memory-model.md).

A known limitation: each top-level dispatch retains a small constant of
persistent slots — the entry slot returned to the user, and, for a bare-name
binding (`LET y = z`), the spliced-out alias slot plus its producer. An aliased
slot is never freed (it has no parent to reclaim it), and a top-level producer
has no parent either. So each `dispatch_in_scope` costs a small constant rather than
one slot — linear in call count, not multiplicative in body size; closing it
would need a post-execute compaction pass.

## Pegged and free execution

Koan code is built once and run many times, but build-time and run-time are
the same engine — the scheduler from this document runs both. The only
difference is that some nodes' results depend on data or effects unavailable
at build time, and those nodes are **pegged** — held without execution
until the data or effect arrives. Build-time runs the scheduler against
the full DAG; nodes that are not pegged execute (and produce values, refine
types, spawn dependents) freely; the run halts at the pegged frontier.
Run-time supplies the inputs and effects, unblocks the pegged nodes, and
the scheduler resumes — same machinery, no new pass.

- **Nodes pegged at build time:** user-supplied input; source files for
  plugins not available at build time; syscalls in builtins; network calls.
- **Nodes that execute freely at build time:** source files available at
  build time; entropy/randomness used for property-test axiom checking and
  cross-implicit equivalence checking.

The intermediate representation is the **stalled DAG state** — the
scheduler's `NodeStore` and `DepGraph` contents at the free-execution
fixed point, plus the identifiers of pegged nodes. Run-time consumes that
state directly: skip parsing, supply the pegged inputs and effects, continue
running the scheduler.

There is no separate type-checking phase preceding evaluation. Inference,
dispatch, and execution interleave in one DAG; build-time is the same
engine running before pegged inputs are unblocked.

