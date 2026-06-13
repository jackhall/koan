# Execution model: graph-based, dispatch separated from execution

Koan's runtime is a three-stage pipeline. Each top-level expression flows through:

```
source ──▶ parse ──▶ dispatch ──▶ execute
        KExpression   KFuture      KObject
```

Dispatch and execution are deliberately separate stages. **Dispatch** does
name-resolution and signature-matching: given a `KExpression` and a `Scope`, it
returns a [`KFuture`](../src/machine/core/scope.rs) — the resolved `&KFunction` plus
its `ArgumentBundle`, ready to run but not yet executed. **Execution** is what
the [`Scheduler`](../src/machine/execute/scheduler.rs) does: it owns a DAG of deferred
work, decides when each `KFuture` runs, and hands its body the live scope.

## Dispatch as a scheduler node

The scheduler models dispatch itself as a node type — `Dispatch(KExpression)`.
[`schedule_expr`](../src/machine/execute/interpret.rs) collapses to "add a `Dispatch`
node per top-level expression"; the rest is dynamic. At run time a `Dispatch`
walks its expression's parts, spawns sub-`Dispatch`/`Bind`/`Combine` nodes for
nested sub-expressions, and a builtin body holding `&mut dyn SchedulerHandle`
can also add `Dispatch` nodes.

`Combine` is the host-side dual of `Bind`: an N→1 combinator that waits on a
fixed set of dep slots and then runs an arbitrary host closure
([`CombineFinish`](../src/machine/core/kfunction.rs)) over their resolved values.
List- and dict-literal planners use it; the construction logic — including
already-resolved literal scalars that don't need a dep slot — lives in the
closure's capture rather than in fixed-shape variants. Body-finalization for
future MODULE/SIG inner work will reuse the same primitive.

`Catch` is the catching dual of a single-dep `Combine`: it waits on one
slot and hands its terminal to a [`CatchFinish`](../src/machine/core/kfunction.rs)
closure as a `Result<&KObject, KError>`. Unlike `Combine`, an errored dep
does not short-circuit — the closure always runs and decides whether to
recover or re-raise. The `TRY-WITH` builtin
([`try_with`](../src/builtins/try_with.rs); see
[error-handling.md](error-handling.md)) is the sole caller today: it
spawns its watched expression as a sub-dispatch and registers a `Catch`
that picks the matching branch by tag.

## The dispatcher / scheduler boundary

The dispatch tree
([`execute/dispatch/`](../src/machine/execute/dispatch.rs)) is a sibling
of [`execute/scheduler/`](../src/machine/execute/scheduler.rs), not
nested inside it. The two communicate through a **decide → outcome →
apply** contract — the dispatch-side peer of the builtin
`Action` / `run_action` split (see [`BodyResult`](#bodyresult--the-three-return-shapes)
below). A dispatch shape handler *decides* against a read-only view and
*returns* its scheduler mutations as data; a harness interprets that data
and is the sole place that holds `&mut Scheduler`. The three pieces:

- **The read view** —
  [`DispatchCx<'run, 's>`](../src/machine/execute/dispatch/ctx.rs) wraps
  `&'s Scheduler<'run>` (never `&mut`). It exposes only the dispatcher's
  reads: the static-over-the-step ones (`current_scope`, `chain_deref`,
  `active_chain`, `build_bare_outcomes`) and the live reads of
  *pre-existing* producers (`is_result_ready`, `would_create_cycle`,
  `read_result`). The `DepGraph`, `NodeStore`, and active-frame fields stay
  `pub(in execute::scheduler)`; the dispatch shape modules (`keyworded`,
  `fn_value`, `single_poll`) never name scheduler fields directly. A future
  scheduler internal rename (`active_chain` → ..., `DepGraph` split) is a
  single-file change inside `scheduler/`.
- **The effect** —
  [`DispatchOutcome<'run>`](../src/machine/execute/dispatch/outcome.rs) is
  the closed set of effects a decide can name (the peer of
  [`Action`](../src/machine/core/kfunction/action.rs)): `Terminal`,
  `Combine` (declare deps + a splice finish), `ParkSelf`, `ParkLift`,
  `Invoke` (run a resolved call), `Redispatch`, `BecomeDispatch`,
  `ElaborateRecordType`. Each is pure data — no `&mut Scheduler` is
  captured.
- **The write harness** —
  [`apply_dispatch_outcome`](../src/machine/execute/dispatch/harness.rs)
  interprets a returned outcome into graph writes and the slot's
  `NodeStep`. It holds the only `&mut Scheduler` on the dispatch side, so no
  decide handler does. The router (`run_dispatch`) builds a `DispatchCx` per
  decide, runs the handler, and hands the outcome to the harness; the
  recent-wakes side-channel drain stays in the router, the legitimate `&mut`
  boundary.

This contract makes `Scheduler` the **sole**
[`SchedulerHandle`](../src/machine/core/kfunction/scheduler_handle.rs)
impl. A builtin invoked mid-dispatch (e.g. `newtype_construct`) routes
through the shared `run_action` harness: `exec::invoke` runs against the raw
`&mut Scheduler` and reads the dispatcher's ambient `current_frame` /
`current_lexical_chain` off it directly to build the builtin's `BodyCtx` —
no `SchedulerHandle` forward, no facade re-borrow.

## `BodyResult` — the three return shapes

A builtin body returns one of:

```rust
BodyResult { Value(&KObject) | Tail(KExpression) | Err(KError) }
```

- `Value` — the body produced a final value; the slot finalizes.
- `Tail` — the body wants to dispatch a fresh expression in its own slot (TCO,
  see below).
- `Err` — structured failure; see [error-handling.md](error-handling.md).

When a body cannot produce its result inline — its expression has nested
sub-expressions whose own evaluation hasn't run yet — the slot's work is
rewritten to `Lift(LiftState::Pending(NodeId))` (a [`NodeWork`](../src/machine/execute/nodes.rs)
variant). The Lift shim parks on the spawned `Bind`'s notify-list; the
notify-walk transitions `Pending → Ready(NodeOutput)` at wake time by
stamping the producer's terminal directly into the Lift's work, so when the
slot pops the terminal is already in hand and `run_lift` just unwraps it —
no result-table lookup. The original slot keeps its frame and notify-list
across the rewrite, so consumers downstream see the eventual terminal as if
the body had produced it directly.

## Push/notify dependency edges

The scheduler's edges point producer → consumer. Each slot carries a
`notify_list: Vec<NodeId>` of dependents waiting on it; each `Bind` /
`Combine` / `Lift` consumer carries a `pending_deps: usize` counter of
unresolved deps. When a slot writes a terminal `Value` or `Err`, the
notify-walk drains its `notify_list`, decrements each consumer's
`pending_deps`, and pushes any zero-counter consumer onto the run-set.
The terminal write and notify-walk fire in a single
[`Scheduler::finalize`](../src/machine/execute/scheduler/execute.rs)
method body that pairs `NodeStore::finalize` with `DepGraph::drain_notify`,
so the "every terminal write fires the notify" rule is type-enforced
rather than restated at each call site. Consumers arrive on the run-set
only when actually ready; there is no poll-and-requeue.

A second fan-out runs alongside the counter-decrement. Each drained
consumer whose work is `NodeWork::Dispatch` (any `DispatchState`
variant) gets the producer's `NodeId` appended to its
`recent_wakes: Vec<NodeId>` side-channel before the counter is
inspected. `Bind` / `Combine` / `Catch` / `Lift` consumers skip the
append — they run a fixed closure on counter-zero and have no
per-edge wake attribution to track. The dispatch driver drains its
slot's `recent_wakes` on entry so the side-channel never grows stale
across re-park; the keyworded and `FunctionValueCall` resume handlers
read the installed track's `subs` Vec directly rather than the wakes
side-channel — at pop time `pending_deps` is zero, so every recorded
sub is terminal. `DepGraph::drain_notify` returns the per-consumer
`hit_zero` flag so the fan-out (always-append plus conditional
stamp-and-enqueue) runs off a single drain.

The run-set has two priority bands managed by
[`WorkQueues`](../src/machine/execute/scheduler/work_queues.rs). Internal
work — notify-walk wake-ups, Replace-arm re-enqueues, and ready-on-arrival
nodes registered in `add()` — routes through `WorkQueues::push_internal` /
`push_internal_front` / `push_woken`. Top-level `add_dispatch` calls route
through `WorkQueues::push_top_level` so independent top-level expressions
execute in submission order. The execute loop drains via `WorkQueues::pop_next`,
which yields internal slots ahead of top-level slots; the routing rule (which
band a push lands in) and the priority rule (which band a pop drains first)
are both enforced by the wrapper's method surface rather than restated at each
call site.

## Dependency graph invariants

[`DepGraph`](../src/machine/execute/scheduler/dep_graph.rs)'s three
parallel vectors — `notify_list`, `pending_deps`, `dep_edges` — share an
index space with `NodeStore::nodes` and uphold three invariants:

- **Inv-A (wake-pending coherence).** For every consumer slot `c`,
  `pending_deps[c] == |{ p : c appears in notify_list[p] }|`. Every
  mutating method on `DepGraph` updates `notify_list`, `pending_deps`,
  and `dep_edges` in a single atomic body so the two fields cannot
  desync.
- **Inv-B (free-cascade source).** `dep_edges[c]` lists every `Owned`
  sub-slot `c` must cascade-reclaim. Park edges are tagged `Notify` and
  filtered out of `free`'s walk via `owned_children`. Independent of
  Inv-A.
- **Inv-C (lazy notify-scrub on free).** A slot `c` is only freed once
  every producer's `drain_notify` has run and removed `c` from
  `notify_list[*]`. The
  `freed_slot_does_not_appear_in_other_notify_lists` test pins this;
  `free` relies on Inv-A and Inv-C still holding rather than scrubbing
  itself.

Inv-B is what makes the eager `dep_edges[idx].clear()` in
`Scheduler::reclaim_deps` sound at `Combine` / `Catch` success: those
slots at reclaim time hold only `Owned` edges (their `deps` / `from`,
all spawned by the slot). `Notify` edges land only on `Dispatch` slots
via the bare-name short-circuit / replay-park in `run_dispatch`, never
on `Combine` / `Catch`, so clearing the list cannot drop a wake intent.

## Lift: push/notify single-producer model

[`NodeWork::Lift`](../src/machine/execute/nodes.rs) exists because the
push/notify model assumes a single producer slot per result. When a
`Dispatch` defers to a `Bind` / `Combine` for sub-deps, it spawns the
worker into a new slot and rewrites its own slot to
`Lift(LiftState::Pending(worker))` so the result still surfaces under
the original slot index. The notify-walk stamps `Pending → Ready` with
the producer's terminal output at wake time, and `run_lift` on pop just
unwraps the stamped `NodeOutput` — no result-table lookup.

The `Pending → Ready` transition is the sole responsibility of
`Scheduler::finalize`. By the time a Lift slot pops, the notify-walk
has already stamped its `LiftState` to `Ready`, so the `Pending` arm of
`run_lift`'s match is a wake-misfire panic that localizes to the
notify graph: reaching it means a Lift was enqueued without its `from`
finalizing — a bug in `Scheduler::finalize`'s stamp or `DepGraph`'s
pending-deps accounting, not in any read-side caller.

## Working-copy splice

The scheduler dispatches each expression by mutating an **owned working
copy** of it. `run_dispatch` extracts every nested sub-expression out of
the parent's `parts` (replacing each with a placeholder `Identifier`) and
declares them as the deps of a
[`DispatchOutcome::Combine`](#the-dispatcher--scheduler-boundary) — its
own dual of a builtin `Combine`. The harness submits each dep as a
sub-Dispatch and parks the parent on a
[`NodeWork::DispatchCombine`](../src/machine/execute/nodes.rs) carrying a
*splice finish* (`KeywordedState` / `FnValueState` ride along as the finish
carrier). When the deps terminalize, that finish runs and writes each
resolved value back into the working copy:
`working_expr.parts[part_idx] = ExpressionPart::Future(value)`. The splice
lives **entirely inside the finish** — the scheduler resolves deps and hands
values back exactly as it does for a builtin `Combine`, learning nothing
about `Future` cells. The assembled `Future`-laden expression then goes
through `resolve_dispatch` as if it had been written with literals.

Source-of-truth ASTs are never mutated. The working copy is cloned from
its source at slot-submission time — the user-fn body executor clones each
body statement onto its slot, `match_case::body` and `try_with` clone their picked arm, top-level
expressions move into the slot at `add_dispatch`. The splice mutates the
slot-owned copy and nothing else; the next call to the same FN clones the
body fresh.

The splice gives typed-slot dispatch a uniform input shape: sub-Dispatch
results land in the same positions as literals would, so the
slot-specificity scoring path is unified across builtins, user-fns, and
pre-evaluated sub-expressions. The cost — body clone per call, one slot
per nested `(...)` — and what it buys are detailed in
[Performance characteristics](#performance-characteristics).

## Tail-call optimization

[`BodyResult::Tail(KExpression)`](../src/machine/core/kfunction.rs) makes a tail
return rewrite the **current scheduler slot's work** to a fresh
`Dispatch(expr)` and re-run in place — no new node allocated. Both deferring
builtins (`match_case`, and `run_user_fn` for user-fns) are tail by
construction. A chain of tail calls (`A → B → PRINT`, or unbounded
`LOOP → LOOP`) reuses one slot end-to-end. Verified by two slot-count
assertions in the test suite.

The slot's `Rc<CallArena>` is held in exactly one place during each step,
which is what lets the tail-reuse path detect "nothing escaped" and reset
the frame shell in place across iterations rather than allocating a fresh
one. See
[per-call-arena-protocol.md § TCO frame reuse](per-call-arena-protocol.md#tco-frame-reuse).

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
a sub-`Dispatch`; the consumer is either the parent `Dispatch` slot
itself (parked as a `DispatchCombine`) or, for
list/dict aggregates and combinator builtins like `TRY`, a `Combine` /
`Catch` slot. Without reclamation those slots accumulate per body
iteration, so realistic recursive code is O(n) scheduler memory even
when its data footprint is O(1).

Reclamation runs at the start of a `DispatchCombine` finish
(`run_dispatch_combine` reclaims deps before the finish, since a dispatch
finish writes its own edges), and at the end of `run_combine` and
`run_catch`. Once the consumer has read its dep results and either spliced
them into `working_expr.parts` as `Future(value)` (the eager-subs splice
finish) or handed them to its finish closure (Combine / Catch), the dep
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
[`NodeStore`](../src/machine/execute/scheduler/node_store.rs)
sub-struct on `Scheduler` that owns four private vectors — `nodes:
Vec<Option<Node<'a>>>` (active node payloads), `results:
Vec<Option<NodeOutput<'a>>>` (terminal results), `free_list: Vec<usize>`
(recyclable indices), and `recent_wakes: Vec<Vec<NodeId>>` (per-consumer
side-channel of producers that have fired since the slot's last poll,
populated only for `NodeWork::Dispatch` consumers) — and the slot
lifecycle that moves each index through them: `alloc_slot → take_for_run
→ reinstall* → finalize → free_one`. Each transition is a single atomic
mutator body, so the recycle-vs-extend choice, the take/reinstall
pairing, the terminal write, and reclamation are each encapsulated; no
call site outside `NodeStore` can grow `nodes` without `results` or land
a `NodeOutput` without firing the notify-walk. `recent_wakes[idx]` is
cleared in O(1) by `free_one` (inner Vec capacity retained for the next
owner) and extended in lockstep with `nodes` by `alloc_slot`'s extend
arm, so every live `NodeId` indexes a valid inner Vec without a separate
growth pattern.
Dependency bookkeeping lives alongside it in a single
[`DepGraph`](../src/machine/execute/scheduler/dep_graph.rs) sub-struct
that bundles three parallel vectors — `notify_list: Vec<Vec<NodeId>>` (each
producer's dependent list), `pending_deps: Vec<usize>` (each consumer's
unresolved-dep counter), and `dep_edges: Vec<Vec<DepEdge>>` (each slot's
backward edges to producers it depends on, tagged `Owned` or `Notify`; the
`Owned` arm carries the ownership tree the free walk follows, and the
`Notify` arm carries park-only edges that the walk skips). The three vectors
are kept private and mutated only through a small surface
(`install_for_slot`, `add_owned_edge`, `add_park_edge`, `drain_notify`,
`owned_children`, `clear_dep_edges`) so every change preserves the tri-vector
invariant atomically — every forward edge in `notify_list[p]` has a matching
backward entry in `dep_edges[c]` and contributes 1 to `pending_deps[c]`.
`Scheduler::add` orchestrates across the two sub-structs: `NodeStore::alloc_slot`
picks the index (popping `free_list` or extending) and `DepGraph::install_for_slot`
branches privately on whether the slot is recycled or freshly extended to
write the dep entries in lockstep. See also
[memory-model.md § Performance notes](memory-model.md).

A known limitation: each top-level dispatch retains two persistent slots —
the entry `Lift` slot returned to the user, and the `Bind` it lifts from
(which the user-fn body writes its terminal `Value` into). Neither has a
parent to free it, so each `add_dispatch` costs a small constant rather than
one slot. Linear in call count, not multiplicative in body size; closing it
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

## Dispatch-time name placeholders

Forward references between sibling top-level expressions, members of a
`MODULE` body, and (eventually) names imported across files all require the
same property: a value- or type-position lookup whose target binder has
dispatched but not yet executed parks on the producer instead of failing with
`UnboundName` — **provided the binding is lexically visible from this
reference's source position.** Visibility is the index gate (see
[Lexical provenance chain](#lexical-provenance-chain) below): every binding
carries the lexical statement index it was registered at, and a consumer at
chain cutoff `c` sees only bindings with index `i < c`. This is one rule
across the value and type languages — there is no per-binding exemption.
Mutual recursion of two or more nominal types, which has no valid source
order, is co-declared in a `RECURSIVE TYPES` block that scopes its threaded
group within strict lexical order (see
[typing/user-types.md](typing/user-types.md)); a self-recursive type threads
its own name and needs no block.

Every binder is value-style gated (strict `b.idx < c`), so a forward
reference to a later-sibling `LET`, `STRUCT`, `FN`, or any other binder is
invisible. A later-sibling `LET` surfaces `UnboundName`; a forward call to a
later-sibling `FN` overload surfaces `DispatchFailed` rather than parking on
the not-yet-finalized overload; a forward type reference is a position error.
A *keyword-headed* function call (`ID 7`) resolves through the
`functions` bucket, which applies the same per-overload visibility filter:
a later-sibling overload registered after this consumer's statement is
hidden, and dispatch falls through to outer scopes. Forward calls from a
function *body* are unaffected — bodies re-dispatch per call against the
body's lexical chain, by which point every sibling binder has registered.

The mechanism lives in two pieces, each routed through a separate install
channel keyed by the binder's shape.

A `placeholders` table — a `RefCell<HashMap<String, (NodeId, BindingIndex)>>`
— lives inside the [`Bindings`](../src/machine/core/bindings.rs) façade
on `Scope` alongside `data`, `types`, `functions`, and
`pending_overloads`. *Name-keyed binders* (`LET`, `STRUCT`, `UNION`,
`SIG`, `MODULE`, `RECURSIVE TYPES`) install through their
[`binder_name`](../src/machine/core/kfunction/body.rs) hook (a per-
`KFunction` extractor of type
[`BinderNameFn`](../src/machine/core/kfunction/body.rs) that pulls the
to-be-bound name structurally out of the expression's parts), stamping
`name → producer NodeId` paired with the binder's
[`BindingIndex { idx }`](../src/machine/core/bindings.rs) — the lexical
statement index, gated by the strict `idx < cutoff` rule like every other
binder.

*Bucket-keyed binders* (`FN`, `FUNCTOR`) install through a
[`binder_bucket`](../src/machine/core/kfunction/body.rs) extractor
([`BinderBucketFn`](../src/machine/core/kfunction/body.rs)) into a
separate `pending_overloads` table — a
`RefCell<HashMap<UntypedKey, Vec<(NodeId, BindingIndex)>>>` keyed by
the inner-call bucket key so a later-arriving call expression can park
on a not-yet-finalized overload. FN/FUNCTOR carry **only** the
`binder_bucket` extractor — no `binder_name` — because sibling
overloads under one head keyword (e.g. two `FN (PICK xs :A) ...` /
`FN (PICK xs :B) ...` declarations) must not collide on a single
`placeholders[name]` slot. The two channels are mutually exclusive per
binder: each binder uses exactly one. The submission walk reifies the
choice as a
[`BinderKey`](../src/machine/execute/scheduler/submit.rs) enum
(`Name(String)` vs. `Bucket(UntypedKey)`) so the dichotomy rides in
the type rather than as a two-Option convention.

The bucket vec is what admits multiple sibling FN/FUNCTOR binders
sharing one bucket key: each install appends a distinct entry at its
own `BindingIndex`. A consumer looking up the bucket via
[`Bindings::lookup_function`](../src/machine/core/bindings.rs) gets the
*earliest-index visible* `pending_overloads[key]` entry in the returned
`FunctionLookup`'s `pending` field — the most-likely-first-finalizer. On
that producer's finalize, only the matching entry is removed from the vec
(others stay pending); the consumer wakes, re-dispatches, and either picks
from the now-live `functions[bucket]` or re-parks on the next-earliest
pending sibling. Each re-dispatch is cheap, and the expected case
(consumer's match lands in the first 1–2 siblings) avoids the cost
entirely.

The six binder builtins (`LET`, `FN`, `STRUCT`, `SIG`, `UNION`,
`MODULE`) opt in via
[`register_builtin_with_binder`](../src/machine/core/kfunction.rs);
everything else stays placeholder-free.

Production reads thread the three-layer
[lookup → admit protocol](typing/lookup-protocol.md): `Scope::resolve_*_with_chain`
walks ancestors, the `Bindings::lookup_*` accessors apply the
`chain_cutoff`-gated `visible` predicate per entry, and `KType`
predicates accept or reject the candidate. The placeholder mechanism
extends the value- and function-side lookups so a still-running visible
producer surfaces as `Resolution::Placeholder(NodeId)` /
`FunctionLookup { pending: Some(_), .. }` rather than `UnboundName` —
[`Bindings::lookup_value`](../src/machine/core/bindings.rs) consults
`data` then `placeholders`, and
[`Bindings::lookup_function`](../src/machine/core/bindings.rs) surfaces
the visibility-filtered `functions[key]` overloads and the earliest-index
visible `pending_overloads[key]` producer *together* in one
`FunctionLookup`. The dispatcher decides each scope's contribution from
that pair as it walks (a visible pending parks the scope; see
[scheduler.md § In-walk dispatch precedence](typing/scheduler.md#in-walk-dispatch-precedence)),
so the bucket / pending-overload pair surfaces from one traversal rather
than two. The
raw map accessors (`data` / `types` / `functions` / `placeholders` /
`pending_overloads`) are gated `#[cfg(test)]`; production sites that
genuinely sweep all members (`MODULE` member mirroring, signature
shape-check, REPL reflection) consume the value-yielding `iter_data` /
`iter_types` / `iter_functions`, which release the underlying borrow at
the iterator boundary. `bind_value` and `register_function` remove their
own placeholder before inserting into `data` / `functions`, so the two
tables are mutually exclusive at any moment.

### Miri Lift-park lifetime contract

The bare-name short-circuit and replay-park routes both park through
`Lift(LiftState::Pending(producer))` (see [Lift: push/notify single-producer
model](#lift-pushnotify-single-producer-model) for the stamping protocol).
When the parked Lift pops with `LiftState::Ready(NodeOutput::Value(obj))`,
the `&KObject<'a>` it carries is the **producer's reference**, not a
clone — the notify-walk stamped the producer's terminal pointer directly
into the Lift's state. The producer's arena therefore must outlive every
wake-and-re-run cycle of every consumer parked through this Lift. The
`lift_park_minimal_program_for_miri` and `replay_park_minimal_program_for_miri`
tests pin the contract under Miri tree borrows.

### Submission-time binder install and recursive sub-Dispatch

[`Scheduler::add_with_chain`](../src/machine/execute/scheduler/submit.rs)
inspects every incoming `NodeWork::Dispatch` against the dispatching
scope's ancestor chain via `extract_binder_install`: it finds the first
overload in the matching `functions[expr.untyped_key()]` bucket whose
`binder_name` OR `binder_bucket` extractor returns `Some(_)` for the
expression. The picked overload's install channel is reified as
`BinderKey::Name(name)` (for `LET` / `STRUCT` / `UNION` / `SIG` /
`MODULE`) or `BinderKey::Bucket(key)` (for `FN` / `FUNCTOR`); the
install site stamps the corresponding `placeholders[name]` or
`pending_overloads[bucket]` entry on the dispatching scope before the
slot is ever popped from the work queues. A later sibling that
dispatches before the binder's slot pops finds the entry and parks
rather than surfacing `UnboundName` / `DispatchFailed`.

For binder-shaped Dispatch nodes, the submission walk also recurses into
the expression's eager Expression-shaped argument slots and submits each
as a sub-Dispatch *at the same outermost submission point*. The walk
computes an `eager_slot_mask` over the bucket — a slot is eager only if
*every* binder overload in the bucket marks it non-`KType::KExpression`;
any overload tagging a slot lazy keeps that slot out of the recursive
walk because the eventual dispatch may resolve to that overload. Lazy
slots — FN body, FN signature/return-type-`KExpression` overload, FUNCTOR
body, MODULE body — dispatch in the callee's scope at body-invoke time,
not here. Each recursive `add_with_chain` runs its own
`extract_binder_install`, so a nested binder's placeholder installs at
the same outermost step as its parent's; recursion terminates at
non-binder leaves and at lazy slots, bounded by AST depth.

The collected `(slot_idx, sub_node_id)` pairs ride through into the
parent's `NodeWork::Dispatch { expr, pre_subs }`
([`nodes.rs`](../src/machine/execute/nodes.rs)). When the parent runs,
the fused splice / park / eager-sub walk in
[`dispatch.rs`](../src/machine/execute/dispatch.rs) consults
`pre_subs` before the `Expression` / `ListLiteral` / `DictLiteral` arms:
a slot already pre-submitted reuses the existing `NodeId` (and replaces
the part with an empty-`Identifier` placeholder for the eventual `Bind`
splice) rather than allocating a fresh sub-Dispatch. The
`KeywordedState::install_bare_name_park` and `install_overload_park`
installers carry `pre_subs` into the `KeywordedState.init.pre_subs`
field of the parked state, and `KeywordedState::resume` hands it back to
`initial` on wake — so a park-and-wake cycle does
not re-allocate the pre-submitted children.

Statement indices are per-`enter_block` call: each call to
[`Scheduler::enter_block`](../src/machine/execute/scheduler.rs) mints
chain frames at indices `1..N` for the N statements it submits. A REPL
or test fixture that submits without an ambient chain (the
[`Scheduler::add`](../src/machine/execute/scheduler/submit.rs) auto-root
branch) gets [`LexicalFrame::detached`](../src/machine/core/lexical_frame.rs)
— a chain that mentions no real scope, so the visibility predicate's
`index_for → None ⇒ complete` arm makes every binding in the target
scope visible. This is what lets a REPL query read through to every
prior bind without sharing an index space with them.

The execute side — [`run_dispatch`](../src/machine/execute/dispatch.rs) —
opens with a pre-walk shape classifier. `classify_dispatch_shape` sweeps the
expression's parts for any `Keyword` first and, if none, branches on the head
token's shape, producing a `DispatchShape` variant. The no-keyword fast-lane
variants run their own handlers and never enter
`Scope::resolve_dispatch_with_chain`: there are no candidates in
`bindings.functions` for these shapes, so the candidate machinery would do no
useful work. The single-part lanes (`BareIdentifier`, `BareTypeLeaf`,
`SigiledTypeExpr`, `RecordType`, `LiteralPassThrough`) surface a name or value directly, while
the multi-part head-position call lanes (`TypeCall`, `FunctionValueCall`,
`HeadDeferred`, `TypeHeadDeferred`) each resolve their head to a callable and
converge on the [shared apply-a-callable tail](#dispatch-time-name-placeholders). A
non-callable multi-part head is `NonCallableHead`, a direct `DispatchFailed`
from the dispatch entry. The `Keyworded` variant — produced only when a real
keyword is present — falls into the chain-walked resolution plus eager
name-resolve plus dep-schedule pipeline below.

The keyworded pipeline runs in four steps. Step 1 builds the bare-name
outcome cache: one
[`resolve_name_part`](../src/machine/execute/dispatch.rs) call per
bare-name part of `expr` (`Identifier` or leaf `Type`) into
`bare_outcomes: Vec<Option<NameOutcome<'a>>>`, with `None` for non-bare-name
parts. The cache is built with `consumer = None` so cycle detection is
deferred to Step 4, where it runs only on slots the picked function
classifies as references (a binder declaration slot like `x` in `LET x = …`
has the dispatching slot as its own placeholder's producer, so an upfront
cycle check would false-positive on declarations). Step 2 sweeps the cache
for `NameOutcome::ProducerErrored`: a bare-name arg whose producer
terminalized with an error can never resolve, so it propagates upfront with
a `<wrap-resolve>` frame before any candidate work.

Step 3 calls
[`Scope::resolve_dispatch_with_chain`](../src/machine/core/scope.rs) once,
passing the cache as `bare_outcomes: &[Option<NameOutcome<'a>>]`. Admission
is strict-only: [`signature_admits_strict`](../src/machine/execute/dispatch/resolve_dispatch.rs)
reads each bare-name slot's cached outcome rather than re-resolving it per
scope. A `Resolved(obj)` cache entry admits iff
[`KType::accepts_part`](../src/machine/model/types/ktype_predicates.rs)
holds for the carried type — a bare name whose value has the wrong carrier
type strict-rejects the overload, and the call surfaces as `DispatchFailed`
rather than a bind-time `TypeMismatch`. `Parked` / `Unbound` cache entries
admit via shape-only `arg.matches(part)`: the post-pick splice/park walk in
Step 4 is the only place that produces precise per-slot `ParkOnProducers` /
`UnboundName` diagnostics, so admission must not reject and lose them. The
match on [`ResolveOutcome`](../src/machine/core/scope.rs) is:
`Resolved(r)` continues into Step 4 with the strict-picked function plus
the per-slot index buckets `r.slots` carries (`wrap_indices`,
`ref_name_indices`, `eager_indices`); `Ambiguous(n)` surfaces as an
`AmbiguousDispatch` error; `Unmatched` surfaces as `DispatchFailed`;
`Deferred` (the candidate may match after sub-evaluation yields a typed
`Future(_)`) routes to `KeywordedState::install_eager_only`, which declares every
eager-shaped part as a `DispatchCombine` dep and parks this slot on them;
the splice finish re-resolves dispatch against the spliced expression at
dep completion;
`ParkOnProducers(_)` and `UnboundName(_)` are decided inside the scope walk
as described below.

`resolve_dispatch_with_chain` decides each visible scope's contribution as
it walks innermost-first, from the finalized overloads and the visible
in-flight pending producer the scope's `FunctionLookup` surfaces together.
The innermost scope to reach a terminal outcome wins; only `UnboundName` and
`Unmatched` are decided post-walk. Per scope:

1. A visible pending sibling parks the scope (`ParkOnProducers`) — it would
   shadow any finalized overload here once it finalizes, so the scope
   resolves nothing until it does, even over a same-scope finalized
   strict-Pick. The wake re-dispatches against the now-registered overload.
2. Otherwise the strict gate Picks the most-specific admitting overload
   (`Resolved`), surfaces a genuine tie (`Ambiguous`), or — on a tie with an
   unevaluated eager part that may break it — `Deferred`.
3. Otherwise (strict-Empty) one relaxed-admission pass per candidate assumes
   every unresolved slot satisfiable and resolves by what each leaned on: a
   `Parked` bare name (a producer exists) ⇒ `ParkOnProducers`; otherwise an
   unevaluated eager part ⇒ `Deferred`; otherwise a `Dead` unbound bare name
   records an `UnboundName` blocker without terminating — an unbound name
   never arrives, so it never parks, and holding it back lets an outer scope
   still strict-Pick the bare name shape-only as an `:Identifier` / `:Any`
   slot.

After the walk: a recorded dead-lean blocker ⇒ `UnboundName(name)`; nothing
contributed even a dead lean ⇒ `Unmatched`. Parked outranks eager (a parked
bare name is just an eager part whose value arrives from a producer), and
eager outranks the dead-lean `UnboundName` because an eager part's evaluation
may itself surface the precise diagnostic — surfacing `UnboundName` first
would pre-empt an Expression-in-Type-slot dispatch (`(maybe) some 42`) whose
head evaluates to the schema after one sub-Dispatch.

The rails the dispatch driver feeds:

- **Fast lane** (pre-walk classifier, runs before any resolve walk).
  `classify_dispatch_shape` is one pass over `expr.parts`: keyword anywhere
  ⇒ `Keyworded` (refined to `OperatorChain` for the chain shape); single-part
  `Identifier` ⇒ `BareIdentifier`; single-part leaf `Type` ⇒ `BareTypeLeaf`;
  single-part `SigiledTypeExpr` ⇒ `SigiledTypeExpr`; single-part `:{…}`
  `RecordType` ⇒ `RecordType`; single-part literal /
  value ⇒ `LiteralPassThrough`. With the no-keyword precondition established,
  a multi-part expression branches on its head: leaf-`Type` head ⇒ `TypeCall`;
  `Identifier` head ⇒ `FunctionValueCall`; nested-`Expression` head ⇒
  `HeadDeferred`; `:(...)` `SigiledTypeExpr` head ⇒ `TypeHeadDeferred`; a
  literal / list / dict / record-literal / record-type head ⇒ `NonCallableHead`
  (a record *type* is a value, not a callable). The "sweep first,
  branch on head second" ordering matters: a mixed shape like `(f IF x)`
  goes to `Keyworded` because only the candidate machinery knows how to
  dispatch the `(_ IF _)` bucket. `Keyworded` is never a catch-all for an
  unclassified head — a non-callable head is its own `NonCallableHead` sink.

  Each fast-lane variant has its own handler:

  - `BareIdentifier` (`(some_var)`) — `single_poll::bare_identifier` consults
    `Scope::resolve_with_chain` against the consumer's `LexicalFrame`:
    `Value` returns a `Terminal` outcome inline, `Placeholder` returns a
    `ParkLift` outcome whose harness rewrites the slot's work to
    `Lift(LiftState::Pending(producer_id))` (the same shim `BodyResult::Tail`
    uses for sub-Bind waits), `UnboundName` falls through to the keyworded
    path so `value_lookup`'s body produces the structured error.
  - `BareTypeLeaf` (`(Number)`, `(IntOrd)`) — `bare_type_leaf`
    routes through `resolve_type_leaf_carrier` over the memoized,
    park-capable `Scope::resolve_type_expr` bridge: a leaf naming an
    earlier still-finalizing binder parks on its producer and re-resolves
    on wake, like every compound type form, and other failures surface
    directly. There is no candidate-machinery alternative for a bare leaf
    type. See
    [typing/elaboration.md § Layers](typing/elaboration.md#layers)
    § Layer 4 for the shared resolver seam.
  - `TypeCall` (`MyStruct {x = 1}`, `MyFunctor {T = IntOrd}`) —
    [`type_call`](../src/machine/execute/dispatch/single_poll.rs)
    resolves the head Type token to its `bindings.types` identity. A
    `SetRef` identity is a `ResolvedCallable::Constructor`; a
    `KType::KFunctor { body: Some }` (a bound functor in the type table) is a
    `ResolvedCallable::Function`. Both flow through the shared
    apply-a-callable tail (below). No value-side carrier is fetched — the
    schema rides the identity. Opaque / Module / unbound heads surface a
    `TypeMismatch`. A head token bound to a still-finalizing producer (a
    forward functor `LET`) parks on it and re-runs `type_call` on resume.
  - `SigiledTypeExpr` (single-part `:(...)` wrapper) — the `run_dispatch`
    arm tail-replaces the slot with a `Dispatch`
    of the wrapped `KExpression`, so the inner expression runs through the
    same classifier and produces the same carrier shape any other dispatch
    site does. See
    [type-language-via-dispatch.md](typing/type-language-via-dispatch.md)
    for the full type-language dispatch contract.
  - `RecordType` (single-part `:{…}` record type) — `record_type` folds the
    field list straight to `KType::Record` through the shared field-list
    elaborator (no tail-replace, no internal type-constructor builtin),
    deferring through a Combine only when a field type forward-references or
    sub-dispatches. See
    [type-language-via-dispatch.md § Record-type sigil](typing/type-language-via-dispatch.md#record-type-sigil).
  - `FunctionValueCall` (`f {x = 7}`) — [`FnValueState`](../src/machine/execute/dispatch/fn_value.rs)
    resolves the `Identifier` head and handles every admission outcome
    directly. The call shape admits iff `expr.parts[1..]` is exactly one
    nested-parens part (the *only* call shape — koan has no `f 1 2`
    positional call syntax for function values, so the named-arg shape
    is the whole user-facing surface). A `KFunction(f, _)` head resolves to a
    `ResolvedCallable::Function` and a `KType::SetRef { .. }` head in the value channel's
    `Type` arm — the identity a value-classified alias of a constructible type
    surfaces (`LET outcome = Outcome` then `(outcome (Err "x"))`) — to a
    `ResolvedCallable::Constructor`, both flowing through the shared
    apply-a-callable tail (below). Any other carrier (number, string, instance
    struct, module, …) surfaces a `TypeMismatch` directly. A `Placeholder` head
    installs the head-placeholder park; an unbound head surfaces
    `UnboundName(name)` directly — this shape never falls through to
    `Keyworded`. Reconstruction errors from
    `KFunction::reconstruct_positional` (missing / unknown /
    duplicate-named args, malformed pair shapes) surface as
    `NodeOutput::Err` with the same structured wording the keyworded
    path produces.
  - `HeadDeferred` (`(pick) {x = 1}`) and `TypeHeadDeferred`
    (`:(MyFunctor {base = IntOrd})`) — [`HeadDeferredState`](../src/machine/execute/dispatch/head_deferred.rs)
    sub-dispatches the head first (an Owned edge; the park/resume pair mirrors
    `CtorState`'s), then branches the resumed value's kind into a
    `ResolvedCallable`. `HeadDeferred` admits a function, functor, bound functor,
    or constructible type; `TypeHeadDeferred` (the `:(...)` sigil guarantees a
    type) prunes the plain-function arm and surfaces a type-shaped `TypeMismatch`
    on a non-type. Both then run the shared apply-a-callable tail.

  **The shared apply-a-callable tail.** All four head-position call lanes —
  `TypeCall`, `FunctionValueCall`, `HeadDeferred`, `TypeHeadDeferred` —
  converge on [`apply_callable`](../src/machine/execute/dispatch/apply_callable.rs).
  A `ResolvedCallable` has exactly two execution arms: `Constructor(&KType)`
  builds from a struct / tagged / newtype / `TypeConstructor` schema, and
  `Function(&KFunction)` calls a `KFunction` by name. A functor is a `KFunction`
  whose result is a module, so functor application is the `Function` arm — the
  functor/function distinction survives only at classification (for `KFunctor`
  typing and the `TypeHeadDeferred` diagnostic gate), never at execution. The
  tail body-shape-branches `expr.parts[1..]` (`extract_call_body` admits one
  `{name = value}` record literal or one `(value)` paren group) and launches
  construction or a `reconstruct_positional` + eager-subs function call. The
  eager-subs stage resolves the reconstructed call's bare-name value slots — the
  `wrap_indices` set from `classify_for_pick` — by sub-Dispatch, the same lane as
  `Expression` / literal args, so each resolves to its `Future` carrier before
  `KFunction::bind`. The committed callable's slot admission (`accepts_part`) then
  runs the carried-type check at bind: a `:Signature` slot consults the witness
  module's `compatible_sigs`, exactly as the keyword-led path does. Because the
  head has already selected the one callable, the keyword path's pre-pick
  `bare_outcomes` resolution (which exists to choose among co-bucket overloads) is
  unneeded here; a genuinely non-satisfying arg is a terminal `TypeMismatch`, not a
  fall-through, since there is no other overload to try.

  Forward references resolve through the fast lane and the eager
  name-resolve rail (below), both of which route name lookups through
  `Scope::resolve_with_chain` against the consumer's `LexicalFrame` and so
  consult the visibility-gated `placeholders` table. A *keyword-headed*
  call — `ID 7`, where `ID` is the head Keyword — dispatches through the
  `functions` bucket, which applies the same per-overload visibility filter
  (see [ktype.md § Overload bucket visibility filter](typing/ktype.md#overload-bucket-visibility-filter)).
  A later-sibling overload registered after this consumer's statement is
  hidden, and dispatch falls through to outer scopes; finding nothing
  surfaces as `DispatchFailed`. Forward calls between sibling FNs work
  through the bucket-keyed `pending_overloads` channel: each sibling FN
  install appends a distinct entry to the per-bucket vec, and a parking
  consumer wakes on the earliest-index visible producer, re-parking on
  the next-earliest if its pick doesn't admit. Forward calls from a
  function *body* are unaffected because bodies re-dispatch per call
  against the body's lexical chain, by which point every sibling binder
  has registered.
- **Placeholder install** (Step 3.5). If the picked function carries a
  `binder_name` extractor, the driver installs `name → NodeId(idx)` into
  `placeholders` on the dispatching scope. If it carries a `binder_bucket`
  extractor, the driver appends a `(NodeId(idx), BindingIndex)` entry
  into `pending_overloads[bucket]` on the same scope. Each binder uses
  exactly one of the two channels — the `BinderKey` enum in
  [`submit.rs`](../src/machine/execute/scheduler/submit.rs) makes the
  dichotomy a type-level fact. Both installs are lenient against the
  matching submission-time install (see [Submission-time binder install
  and recursive sub-Dispatch](#submission-time-binder-install-and-recursive-sub-dispatch)
  below) — a `(name, idx)` pair already installed at submission re-applies
  cleanly here, and a bucket entry already appended at submission is not
  re-appended. A `Rebind` collision on the name channel against a
  different producer surfaces as a `Done(Err(_))` step so other slots
  keep draining; bucket-channel installs never Rebind (sibling appends
  are the intended shape).
- **Fused splice / park / eager-sub walk** (Step 4). One iteration over
  `expr.parts` co-handles the three per-slot rails the strict pick
  carries: wrap-slot splice (`resolved.slots.wrap_indices`), ref-name-slot
  park (`resolved.slots.ref_name_indices`), and eager sub-Dispatch
  scheduling (filtered by `resolved.slots.eager_indices` when the picked
  function is a lazy candidate, otherwise every eager-shaped part
  schedules). Per part, exactly one arm fires.

  Wrap and ref-name arms read the same `bare_outcomes[i]` cache the
  resolver consumed in Step 3 — so each bare name is resolved once per
  `run_dispatch` invocation, shared across admission and the walk.
  Per-arm behavior:

  - **Wrap slot.** `Resolved(obj)` rewrites the slot to
    `ExpressionPart::Future(obj)` in place. `Parked(p)` cycle-checks
    via [`DepGraph::would_create_cycle`](../src/machine/execute/scheduler/dep_graph.rs)
    and either surfaces `SchedulerDeadlock { sample: "cycle in type alias
    `<name>`" }` on a self-park or pushes `p` onto the shared
    `producers_to_wait` list. `Unbound(name)` surfaces a slot-terminal
    `UnboundName` (the parent binder's Combine reads it through
    `read_result(dep)` and short-circuits with the right framing — an
    `Err` from `execute` would break that catch).
    `Cycle` / `ProducerErrored` are unreachable here: the cache is built
    with `consumer = None`, and the Step 2 sweep already short-circuited
    `ProducerErrored`.
  - **Ref-name slot.** Literal-name slots keep the bare token, so
    `Resolved` and `Unbound` are no-ops. `Parked(p)` runs the same
    cycle-check then push as the wrap arm. Only `Identifier` and leaf
    `Type` parts park here; non-bare-name parts are skipped by
    classification.
  - **Eager-sub slot.** `Expression` parts sub-Dispatch; `SigiledTypeExpr`
    and `RecordType` parts wrap into a single-part `KExpression` and
    sub-Dispatch (the sub-Dispatch enters `run_dispatch`'s matching shape arm —
    `SigiledTypeExpr` tail-replaces with the inner dispatch, `RecordType` folds
    to `KType::Record`); `ListLiteral` and `DictLiteral`
    route through `schedule_list_literal` / `schedule_dict_literal` for the
    aggregate Combine; any other shape rides through unchanged. Lazy
    `Expression` parts in `KExpression` slots are filtered out by
    `eager_indices` and the receiving builtin dispatches them itself.

  **Park-precedence guard.** Sub-Dispatch and aggregate scheduling are
  staged into a `PendingSub` vec rather than submitted eagerly during the
  walk. After the loop, if `producers_to_wait` is non-empty the decide
  returns through `KeywordedState::install_bare_name_park` as a
  `DispatchOutcome::ParkSelf` — the harness installs the park edges as
  `Notify` (via `add_park_edge`) and transitions the slot to
  `KeywordedState` with the bare-name-park track set, dropping
  `NodeWork::Dispatch.expr` to a placeholder so the state-carried
  `working_expr` becomes the source of truth on wake — **without**
  submitting any staged subs. Eager submission would
  leak the sub-nodes on the re-Dispatch wake path, where the new
  `run_dispatch` invocation would re-stage them.
  Multi-name forward references compose as one combined park rather than
  N independent sub-Dispatches.

  If no producer parked, the driver applies each `PendingSub`: `Reuse(id)`
  for slots already pre-submitted recursively at outermost-submission time
  (see [Submission-time binder install and recursive
  sub-Dispatch](#submission-time-binder-install-and-recursive-sub-dispatch)),
  `Dispatch(sub_expr)` for a fresh sub-Dispatch, and `ListLit` / `DictLit`
  for the aggregate. With no subs to schedule the driver binds the picked
  function directly: the decide returns a `DispatchOutcome::Invoke` whose
  harness runs `dispatch::exec::invoke` (a wrap-slot-only call like
  `MAKESET IntOrd` resolves bare names in Step 4, leaves no eager parts, and
  binds in one step — no Combine detour). Otherwise the decide returns a
  `DispatchOutcome::Combine` declaring the fresh subs as deps with a splice
  finish; the harness parks the slot as a `DispatchCombine` carrying the
  finish on its `KeywordedState`. At dep completion the finish re-resolves
  the spliced `working_expr` and routes it — `Invoke` on the
  speculatively-picked function, or `Redispatch` through
  `KeywordedState::finish` when none was pre-picked.

  Dict and list literals (`classify_aggregate_part` in
  [`scheduler/literal.rs`](../src/machine/execute/scheduler/literal.rs))
  ride the same name-resolve rail when their `wrap_identifiers` plan-input
  is set: bare-name entries call `resolve_name_part` directly and
  materialize as `Slot::Static` (resolved) or `Slot::Park(i)` (parked
  producer), with the Combine driving a single wake across all parked
  siblings.

`Resolved.slots`'s three index vectors (`wrap_indices` / `ref_name_indices` /
`eager_indices`) are disjoint by construction: each slot's
`(SignatureElement, ExpressionPart)` shape lands in at most one bucket.
[`KFunction::classify_for_pick`](../src/machine/core/kfunction.rs) is
the sole producer of the `ClassifiedSlots` carrier (which `Resolved` holds
by value), so the disjointness invariant lives in one place rather than as
comment-enforced rules across the scheduler driver. Cycle detection runs
inside the fused walk (not in the cache build) so it sees the picked
function's slot classification: a binder declaration slot — `x` in
`LET x = …`, `Foo` in `STRUCT Foo (…)` — is owned by the binder, never
classified as wrap or ref-name, and so never reaches the cycle-check arm.
`DepGraph::would_create_cycle` walks the forward `notify_list` graph from
the consumer; if the producer is reachable, the driver surfaces
`SchedulerDeadlock` on the slot terminal instead of installing a park edge
that would close the cycle. That catches the trivially-cyclic
`LET Ty = Ty` / `LET x = x` shapes uniformly — both Identifier-LHS and
Type-LHS cycles surface with the same error kind without a special case
in the elaborator.

The fast-lane handlers (`single_poll::bare_identifier`, the `fn_value`
`FunctionValueCall` head) and the eager-resolve pass return park outcomes
(`ParkLift` / `ParkSelf`) whose harness calls
`DepGraph::add_park_edge`, which records a `DepEdge::Notify(producer)` in
the consumer's `dep_edges` entry alongside the `DepEdge::Owned(child)`
entries that mark sub-slots the consumer owns. `add_park_edge` and its
`add_owned_edge` sibling each install the forward `notify_list[producer]`
wake and the `pending_deps[consumer]` bump atomically with the backward
record, so a park-edge install is one atomic +1 across the three vectors.
`free()` recurses only into `Owned` arms, so a consumer's reclamation
cannot transit a park edge into a sibling producer's subtree. Same-scope
rebind of a value name surfaces as `KErrorKind::Rebind`; an `FN` overload
duplicating an existing exact signature surfaces as
`KErrorKind::DuplicateOverload`. Type bindings share this placeholder
mechanism: a type-binding site registers in `Scope::placeholders` exactly
like a value binding, external lookups park the same way, and
self-references during a binding's own elaboration short-circuit through
the elaborator's threaded-set recognition (see
[typing/elaboration.md](typing/elaboration.md)) so recursive type
definitions don't deadlock on their own placeholder. FN-signature
elaboration plugs into the same mechanism: when
[`elaborate_type_expr`](../src/machine/model/types/resolver.rs) hits a
bare type-name leaf whose binder is in `Scope::placeholders` but not yet
finalized, it returns `ElabResult::Park(producers)` and FN-def's body
schedules a `Combine` over those producers that re-runs the signature
elaboration against the now-final scope at finish time. (See
[typing/elaboration.md § Layers](typing/elaboration.md#layers) § Layer 3
for the elaborator's role in the pipeline.) A parens-wrapped
parameter type (`xs :(LIST OF Number)`) rides the same Combine:
`parse_fn_param_list` records the `(slot_idx, sub_expr)` pair, FN-def
schedules each sub-expression as its own `Dispatch`, and the Combine's
finish closure splices each result into
`signature_expr.parts[slot_idx]` as `Future(Carried::Type(_))` before
re-running the parameter-list walk against the spliced signature. STRUCT
and UNION share the same elaborator-and-Combine shape for their
field-type lists. The fused walk's per-park cycle check
([`DepGraph::would_create_cycle`](../src/machine/execute/scheduler/dep_graph.rs),
covered above) handles the simple trivially-cyclic cases proactively; the
elaborator's threaded-set carry-through handles the recursive-type cases
during STRUCT / UNION body elaboration.

A drain-end guard catches any cycle the proactive check doesn't: after
[`execute`](../src/machine/execute/scheduler/execute.rs) empties its work
queues, it scans the slot table for nodes still parked (`PreRun`) — a
node parked on a dependency that can no longer fire — and returns
`KErrorKind::SchedulerDeadlock { pending, sample }` rather than letting
the top-level result read panic on an unresolved slot. `sample` is the
source expression of the first parked `Dispatch`/`Bind` node, so the
diagnostic points at code the reader can act on.

### `DispatchState` — per-variant state envelope

Every `NodeWork::Dispatch` slot carries a
[`DispatchState`](../src/machine/execute/dispatch.rs) value
that records where the slot is in the per-shape state machine. The enum
has one variant per `DispatchShape` plus a pre-classification birth
state:

```text
DispatchState ::= Initialized(Initialized)
                | BareIdentifier(BareIdState)
                | BareTypeLeaf(BareTypeState)
                | TypeCall(Box<CtorState>)
                | FunctionValueCall(Box<FnValueState>)
                | HeadDeferred(Box<HeadDeferredState>)
                | LiteralPassThrough(LitState)
                | SigiledTypeExpr(SigilState)
                | Keyworded(Box<KeywordedState>)
```

`HeadDeferred` is shared by the `HeadDeferred` and `TypeHeadDeferred` shapes —
the state's `type_only` flag selects the admitted-arm set on resume.

Every per-variant struct embeds the `Initialized` birth state by value
as its `init` field, so any state-carried data (today only `pre_subs`
from the recursive-binder-submission optimization) rides along
structurally without each variant restating the field. The submission
walk hands `Initialized { pre_subs }` to the slot at install time;
`run_dispatch` reads the field on first entry, classifies via
`classify_dispatch_shape`, and transitions to the matching per-variant
struct via a `from_init` / `with_*` constructor that consumes the
birth state. Variants that don't yet carry borrowed state hold the
lifetime with a `PhantomData<&'a _>` marker so additional fields can be
added without churning every pattern site in `execute.rs` /
`submit.rs` / `dispatch.rs`.

The single-poll fast-lane variants (`BareIdentifier`, `BareTypeLeaf`,
`SigiledTypeExpr`, `LiteralPassThrough`) terminalize or single-producer-park in
one poll, so their state structs carry no post-classification tracks. The
variants that re-enter from a parked track — `Keyworded`, `FunctionValueCall`,
`TypeCall` (parked on eager-subs or a still-finalizing head), and `HeadDeferred`
(parked on its head sub-dispatch) — carry the per-shape track they resume from.
`Keyworded` and `FunctionValueCall` hold an `Option<Track>` field per park shape;
the `with_*` constructors install exactly one. These variants are boxed because their multi-track shapes
would otherwise push every `DispatchState`-carrying type
(`NodeWork::Dispatch`, `NodeStep::Replace`, `Node`, `SlotState`) past
clippy's `large_enum_variant` threshold; boxing costs one allocation
per parked slot — a rare path, since the fast-lane variants never
construct these and one-shot paths terminalize without installing a
track.

`Keyworded` carries `init` plus an `Option<ParkTrack>` — `None` on
initial entry, `Some` once the slot parks. `ParkTrack` is an enum of two
mutually-exclusive park reasons (a single resolve either parks on producers
before the part walk, or runs the walk and discovers bare-name producers).
**Eager subs do not park here**: a `Deferred`/eager-subs resolve returns a
[`DispatchOutcome::Combine`](#the-dispatcher--scheduler-boundary) and parks
as a `DispatchCombine` whose finish re-resolves the spliced expression — so
a `Keyworded` resume never re-enters for them. Re-resolve in the finish is
authoritative: an element-typed `Future(_)` that narrows a typed-slot
admission rules a speculative initial pick out, and the call surfaces
`DispatchFailed` (non-match) rather than committing and surfacing a bind-time
`TypeMismatch`.

- **`ParkTrack::BareName(BareNameParkTrack)`** — installed by
  `KeywordedState::install_bare_name_park` when the part walk discovers ≥1
  `NameOutcome::Parked(producer)` on a wrap or ref-name slot. Park
  edges are installed as `Notify` (via `add_park_edge`) — the
  producers are sibling forward references, not children of this
  slot, so the slot's reclaim walk must not transit into them. Resume
  re-enters `initial` against the carried (partly-spliced) `working_expr`;
  the bare names now resolve through `scope.resolve_with_chain` to
  bound values, so the rebuilt `bare_outcomes` picks them up and the
  wrap-slot splice fires `Future(obj)` on the second pass.
- **`ParkTrack::Overload(OverloadParkTrack)`** — installed by
  `KeywordedState::install_overload_park` when
  `resolve_dispatch_with_chain` returns `ParkOnProducers` before the
  part walk runs — either because a bare-name arg resolved to a
  still-pending `Placeholder`, or because an innermost-visible
  `pending_overloads[key]` entry from a sibling FN / FUNCTOR binder
  is in flight. The track carries the original (unspliced)
  expression, which resume hands back to `initial` on wake to rebuild
  `bare_outcomes` and re-run the resolve against the now-populated
  bucket.

`FunctionValueCall` (`FnValueState`) carries only a head-placeholder park
track — its eager subs route through the shared
`apply_callable::install_eager_subs_track`, which returns a Combine outcome
carrying the picked `KFunction` from the head `Resolution::Value` arm
directly. `FunctionValueCall` is non-overload-set (the head resolves to a
single carrier, not a candidate bucket), so a typed `Future(_)` an eager sub
reveals can't narrow to a more specific pick, and the finish binds `picked`
without re-running `resolve_dispatch`. The head-placeholder park itself is
installed by `fn_value`'s `install_head_park` (a `ParkSelf` outcome) when
the head identifier resolves to `Resolution::Placeholder(producer)`; its
state carries the original (unspliced) call expression, and resume re-runs
the fast lane against it once `scope.resolve_with_chain` lands in the
`Resolution::Value` arm.

**Park exclusivity holds by construction.** A single resolve reaches exactly
one park installer: the overload park installs from a resolve failure
*before* the part walk runs, so no sibling track has been staged; the
bare-name park installs *before* any eager sub could stage, because the part
walk's park-precedence guard runs first (eager submission on the park path
would leak sub-nodes on the re-Dispatch wake). Eager subs never park as a
`Keyworded`/`FnValue` track at all — they take the `DispatchCombine` route —
so the `Option<ParkTrack>` carries at most one reason per slot.

The state is `pub(in crate::machine::execute)` rather than `pub(super)`
because `nodes.rs` (which carries the `NodeWork::Dispatch { state }`
variant) lives at `crate::machine::execute::nodes`, sibling to the
`dispatch/` and `scheduler/` subtrees. The wider visibility is the
minimum needed for `NodeWork` to name `DispatchState`; no caller
outside the execute tree sees the carrier.

The drain-end cycle-detection guard (`NodeStore::unresolved`)
summarizes parked slots from the state-carried expression rather than
`NodeWork::Dispatch.expr`. The Track installers drop the `Dispatch.expr`
field to an empty placeholder once the slot transitions to a parked
variant, so `DispatchState::parked_carrier_expr` walks each variant's
`Option<Track>` fields in install-precedence order to return the
expression the user-facing diagnostic should sample.

## `KObject` and the model/core boundary

[`KObject`](../src/machine/model/values/kobject.rs) is the universal
runtime value type — the `Object` arm of the scheduler's value currency
[`Carried`](../src/machine/model/values/carried.rs); a type rides the
`Type` arm as a raw `&KType`, with no `KObject` box. Pure-data variants
(`Number`, `KString`, `Bool`, `List`, `Dict`, `KExpression`, `Tagged`,
`Record`, `Null`) carry no references into
[`machine::core`](../src/machine/core.rs). The runtime-reference
variants do — `KFunction`, `KFuture`, and `Wrapped`
embed `&'a KFunction<'a>`, `KFuture<'a>`, `&'a KType`, and an
`Option<Rc<CallArena>>` lifecycle anchor. (A module / signature value
travels the `Type` arm as `KType::Module { &Module, .. }` /
`KType::Signature { &Signature, .. }`, so those references live on `KType`,
not `KObject`.) These references are why `model::values::kobject`
imports from `core::{arena, kfunction, scope, scope_id}`.

The references are structural, not incidental. Three hot consumers
read the concrete runtime shape directly:

- [`lift.rs`](../src/machine/execute/lift.rs) compares
  `f.captured_scope().arena` and `m.child_scope().arena` against the
  dying frame to decide whether a per-call function or module needs
  its `Rc<CallArena>` anchor cloned onto the lifted value — `lift_kobject`
  for the `Object` arm, `lift_ktype` for a `Type`-arm module/signature.
- [`KObject::ktype()`](../src/machine/model/values/kobject.rs)
  reports each value's runtime tag, while a `Type`-arm carrier *is* its own
  `KType` identity — a module value reports `KType::Module { module, .. }`,
  a signature value reports `KType::Signature { sig, .. }` — so the
  dispatcher reads the same identity the carrier holds rather than
  a synthesized shadow.
- `Parseable::summarize` and `deep_clone` recurse into the variants
  and read `f.summarize()`, `m.path`, `s.path`, etc. — both methods
  are part of `KObject`'s contract with `Parseable`, which the value
  layer already implements.

Indirecting these through a trait, an opaque handle, a generic
parameter, or a model/runtime split each fail the same way: the
recursive composite variants (`Tagged.value: Rc<KObject>`,
`List.items: Rc<Vec<KObject>>`, `ExpressionPart::Future(&'a KObject)`)
re-form the union at every nesting level, and the hot consumers
need the concrete arena/scope/path identity that the abstraction
would have to expose anyway. The cleanest available shape is the
present one: the model/core boundary is one-way for pure value
types (e.g. `KKey` returns `Result<KKey, String>` rather than
naming `KError`), and the runtime-reference variants of `KObject`
sit on the boundary by necessity, naming the `core` types they
genuinely need.

## Performance characteristics

The slot-based scheduler trades constant-factor speed for behaviors a
recursive tree-walker can't get cheaply.

### Where time goes

- **Per AST node touched.** Each nested `(...)` becomes its own slot.
  Cost: `NodeStore::alloc_slot` (pop a free-list index or extend three
  parallel vectors), `DepGraph::install_for_slot` (write a `dep_edges`
  entry + bump `pending_deps` on the parent + push into the producer's
  `notify_list`), and a work-queue push. On the consumer side, the
  symmetric drain: terminal write, `drain_notify`, decrement counters,
  push the woken consumer onto the run-set. Compared to a recursive
  function call on a `&KExpression`, this is roughly an order of
  magnitude more bookkeeping per node.
- **Per user-fn call.** The body executor clones each body statement onto
  its own slot (over the parts vector) so the slot has its own
  working copy for [the splice mechanism](#working-copy-splice).
  Clone cost is O(body size). It also acquires a per-call frame —
  either reusing the prev-step's `CallArena` shell via
  `try_reset_for_tail` (see
  [per-call-arena-protocol.md § TCO frame reuse](per-call-arena-protocol.md#tco-frame-reuse))
  or allocating a fresh one. The reuse path is allocation-free; the
  fresh path heap-allocates one `Rc<CallArena>` plus six
  `typed_arena::Arena::new()` pools.
- **Per dep-result splice.** O(1) write into `expr.parts`.
- **Per terminal.** Single `notify_list` drain. The cost scales with
  the producer's dependent count, which is typically 1 (the consumer
  parked on it through a `DispatchCombine` or a `Combine`) but unbounded
  in principle (forward-reference parks).

### What amortizes

- **Slot recycling.** `Scheduler::reclaim_deps` frees sub-slots eagerly
  during a `DispatchCombine` finish / `run_combine` / `run_catch`, and `add()`
  pulls
  from the free-list before extending the underlying vectors. A
  steady-state recursive body reuses the same slot indices across
  iterations; `body_subexpression_slots_recycle_across_calls` pins the
  bound at ≤3 net slots/call.
- **Tail-call slot rewrite.** `BodyResult::Tail` rewrites the current
  slot's work in place rather than allocating a new one — one slot
  for an arbitrarily deep tail-call chain.
- **Tail-step frame reuse.** When the prev step's `CallArena` is
  uniquely owned, `try_reset_for_tail` swaps its inner `RuntimeArena`
  for a fresh one and re-binds — no `Rc<CallArena>` box allocation,
  no `Scope` re-anchoring through the heap. See
  [per-call-arena-protocol.md § TCO frame reuse](per-call-arena-protocol.md#tco-frame-reuse).

### Vs a tree-walking interpreter

A recursive descent on `&KExpression` would skip the slot table, edge
bookkeeping, and body clone — probably 5-10× faster on tight numeric
loops. What it can't do cheaply:

- **TCO.** Direct recursion grows the host stack; the koan model
  rewrites a slot in place. A tree-walker needs explicit trampolining
  with a worklist (which is roughly the slot table reinvented).
- **Forward references.** `LET y = (x); LET x = …` parks `y`'s
  sub-Dispatch on `x`'s producer via `Resolution::Placeholder` and
  wakes when `x` finalizes. A tree-walker would need a pre-pass to
  resolve names or fail on out-of-order definitions.
- **Replay-park on pending types.** Type-elaboration can suspend on a
  not-yet-finalized type, rejoin when it lands, and re-run the
  dispatch — without re-evaluating already-computed sub-expressions or
  blocking the host thread.
- **Reclaim semantics.** Transient sub-slots free as soon as their
  parent has consumed them. A tree-walker's stack frames can't
  selectively reclaim mid-call; everything dies together at function
  return.
- **Unified dispatch model.** Slot-specificity scoring runs through
  one `resolve_dispatch` path for builtins, user-fns, and
  pre-evaluated sub-expression results (`Future(&KObject)` typed-slot
  inputs). A tree-walker would need separate evaluation rules for
  literals, arguments, and intermediate results.

The constant factor is the price; the behaviors above are what bought
it.

## Lexical provenance chain

Every dispatched node carries an immutable
[`LexicalFrame`](../src/machine/core/lexical_frame.rs) recording its
position in the source-level block nesting:

```rust
struct LexicalFrame {
    scope_id: ScopeId,
    index: usize,
    parent: Option<Rc<LexicalFrame>>,
}
```

The head is the innermost enclosing block; the chain walks outward
through every enclosing lexical block; `parent: None` at the tail marks
a top-level statement. Sibling statements in the same block share their
`parent` `Rc` (cactus sharing), so the chain is constant-space per
sibling on top of the shared spine.

### Single entry point: `Scheduler::enter_block`

Every dispatched node has a chain because every new lexical block is
entered through one primitive. `Scheduler::enter_block(scope_id,
statements, scope)` prepends a frame `(scope_id, i)` for each
statement `i` onto the current ambient chain and submits the
statements as dispatch nodes:

- Top-level statements
  ([`interpret`](../src/machine/execute/interpret.rs)) enter through
  `enter_block(root.id, exprs, root)` against an empty parent chain.
- `MODULE` and `SIG` bodies enter through
  [`enter_body_block`](../src/machine/core/kfunction/scheduler_handle.rs),
  which delegates to `enter_block`.
- FN, FUNCTOR, MATCH-arm, and TRY-arm bodies split via the shared
  [`split_body_statements`](../src/machine/core/kfunction/body.rs) helper
  (same all-`Expression` rule that `enter_body_block` uses) — the first
  N-1 statements submit as siblings into the body / arm scope at chain
  indices `1..N-1`, and the FN-slot / MATCH-slot / TRY-slot tail-replaces
  into the last statement at index `N` via
  [`BodyResult::tail_with_frame_at_index`](../src/machine/core/kfunction/body.rs)
  or [`BodyResult::tail_with_block_at_index`](../src/machine/core/kfunction/body.rs).
  TCO is preserved on the last statement. Single-statement bodies pass
  through at index 0.
- FN bodies route through `run_user_fn` (see below — the chain
  shape is special because the call site's chain is not the body's
  lexical chain).

The "every dispatched node has a chain" invariant is a debug
assertion in the strict
[`Scheduler::add_with_chain`](../src/machine/execute/scheduler/submit.rs)
path; the public `add` path auto-roots a chain when no ambient one is
present via [`LexicalFrame::detached`](../src/machine/core/lexical_frame.rs)
(so REPL-style submissions outside `enter_block` see every prior bind
in the target scope).

### Multi-statement FN body split

A user-fn body of the shape `((s_0) (s_1) ... (s_{N-1}))` is split at
[`run_user_fn`](../src/machine/core/kfunction/exec.rs) time (via
`body_statement_refs`). The first
`N-1` statements submit as **sibling sub-slots** in the per-call body
scope at chain indices `1..N-1`, and the FN's slot **tail-replaces into
`s_{N-1}`** at index `N` — so TCO is preserved on the terminal statement.
Single-statement bodies pass through at index 0 (no split needed).

Effect ordering between siblings is **topological** (sub-slot scheduling),
not strict source-order: a sibling reads through the index gate
(`b.idx < c`) and can read any earlier sibling's binding, but the
scheduler is free to interleave their executions when their dependency
sets allow it. Backward references across siblings work — a `LET b =
(a)` at index `i` sees a `LET a = …` at index `j < i` — because the
visibility predicate admits the earlier sibling's binding at the
consumer's cutoff. `match_case` arms and `TRY` arms ride the same split
through `BodyResult::tail_with_frame_at_index` /
`tail_with_block_at_index` (see [Single entry point: `Scheduler::enter_block`](#single-entry-point-schedulerenter_block) above).

### FN-body chain assembly

A function's body chain depth must equal the **lexical** nesting of
its definition site, not the **call** depth — otherwise tail-recursion
and mutual tail-recursion would grow the chain without bound.
[`assemble_body_chain`](../src/machine/core/lexical_frame.rs) walks
the FN's captured `outer` scope chain (the lexical-definition path
set up by `CallArena::new`) and, for each enclosing scope, looks it
up in the **call-site** chain via `LexicalFrame::index_for`. Hits
become frames; the result is prepended with the body's own
`(body_scope.id, body_index)` head — `body_index = 0` for single-
statement bodies, `N` for the multi-statement tail-into-last path so
the last statement's cutoff admits every earlier sibling. Misses
("this enclosing lexical block is not on the call-site chain — it has
already returned") drop out of the chain rather than adding frames.

A tail-recursive FN therefore produces an identical-shape chain on
every iteration; a non-tail recursive call does the same; mutual
tail-recursion across two FNs produces chains bounded by the lexical
depth of whichever body is currently dispatching, not the call
stack's depth.

### Arms as own blocks

Each `MATCH` arm and each `TRY` body / `WITH` arm submits through
`enter_block` against a fresh `child_under` scope. The structural
consequence: a `LET` inside a `TRY` body binds into the arm-local
scope and does not survive past the `TRY` (test:
[`try_body_let_not_visible_after_try`](../src/builtins/try_with/tests.rs)).
This closes the **divergent-bind hazard** at the source level — a
binding visible only on one arm's runtime branch can't leak into the
enclosing block where its visibility would depend on which arm fired.

The **divergent-result hazard** is closed symmetrically on the result
side. `MATCH <v> -> :T WITH (...)` and `TRY (<e>) -> :T WITH (...)` carry
a mandatory declared return type `T` that every arm agrees on. The
selected arm tail-replaces carrying a
[`ReturnContract::Arm`](../src/machine/core/kfunction/body.rs) on the
slot, and when its value lifts the scheduler's Done arm checks it against
`T` — [`TypeMismatch`](../src/machine/core/kerror.rs) with a `<return>`
arg on a miss — then re-tags it to `T` so a downstream consumer dispatches
on the declared shape regardless of which arm ran. Enforcement is runtime
and per-arm (the arm that runs is the arm that's checked), the same
discipline FN return types follow — see
[typing/ktype.md § Function signatures](typing/ktype.md#function-signatures).
`ReturnContract`
is the slot's return carrier: `Function(&KFunction)` for an FN / builtin
call, `Arm { ret, kind }` for a function-less MATCH / TRY arm.

### Read-side hook

The chain is read by name resolution through
[`LexicalFrame::index_for(scope_id)`](../src/machine/core/lexical_frame.rs):
the lookup primitive that returns the consumer's statement index in a
given scope (or `None` when that scope is not on the chain — "already
returned", visibility unconstrained). The
[`Bindings::visible`](../src/machine/core/bindings.rs) predicate consumes it as
`b.idx < cutoff` — one rule across the value and type languages; the
value-side `Scope::resolve_with_chain`, the type-side `resolve_type_with_chain`, the
bare-identifier `lookup_with_chain`, and the per-scope
[`Bindings::lookup_value`](../src/machine/core/bindings.rs) /
`lookup_type` / `lookup_function` lookups (the last covering both the
overload-bucket filter and the in-flight `pending_overloads` fall-through
in one pass) all filter through it. The gate is `chain = None`-bypassed
for test fixtures and builtin-registration paths.

## Open work

- **Inference and search as scheduler work**
  ([typing/scheduler.md](typing/scheduler.md)).
  Type inference and modular-implicit resolution reduce to the existing
  `Dispatch` and `Bind` machinery — type-returning builtins on the value
  path, `Bind` as the refinement-and-wake-up mechanism, and stage 5
  implicit search as a single `SEARCH_IMPLICIT` builtin rather than a new
  node kind. Higher-kinded slots and sharing constraints layer on top of
  the scheduler-driven elaborator (see
  [typing/](typing/README.md));
  [stage 5](../roadmap/predicate_typing/modular-implicits.md) layers
  implicit search.
- **Monadic side-effect capture**
  ([roadmap/monadic-side-effects.md](../roadmap/libraries/monadic-side-effects.md)).
  `Scope::out` is one ad-hoc effect channel today; future effects (IO, time,
  randomness) need a uniform carrier that threads through the same node graph.
- **Unified scheduler interface**
  ([roadmap/refactor/unify-scheduler-interface.md](../roadmap/refactor/unify-scheduler-interface.md)).
  Collapse `SchedulerHandle`, `DispatchCx`, and the raw harness writes onto one read-only view
  in / three-way `Done` · `Continue` · `ParkThenContinue` outcome out, with the harness as sole
  graph writer; folds in the fire-and-forget-leading-statement TCO fix.
