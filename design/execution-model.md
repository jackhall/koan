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

The scheduler models dispatch itself as a node. There is one node shape — a
[`NodeWork`](../src/machine/execute/nodes.rs) struct that waits on a set of deps
and then runs a [`NodeCont`](../src/machine/execute/outcome.rs) closure over
their resolved terminals. A top-level expression enters as a *dispatch decide*: a
`NodeWork` whose `cont` classifies the expression on first poll
([`schedule_expr`](../src/machine/execute/interpret.rs) collapses to "add one
dispatch decide per top-level expression"; the rest is dynamic). At run time a
decide walks its expression's parts, spawns sub-dispatch nodes for nested
sub-expressions, and a builtin body can declare further dispatch nodes as deps of
the `Outcome` it returns.

Per-family behavior — combine vs. catch vs. decide — is not a node variant; it is
which combinator built the `cont` closure ([`short_circuit`](../src/machine/execute/outcome.rs)
/ [`catch_cont`](../src/machine/execute/outcome.rs) /
[`ignore_results`](../src/machine/execute/outcome.rs) in
[`outcome.rs`](../src/machine/execute/outcome.rs)). The node itself never branches
and names no AST.

- A **combine** `cont` (built by `short_circuit`) waits on a fixed set of dep
  slots, short-circuits on the first errored dep, and otherwise runs an arbitrary
  host closure ([`CombineFinish`](../src/machine/execute/outcome.rs)) over their
  resolved values. List- and dict-literal planners use it; the construction logic
  — including already-resolved literal scalars that don't need a dep slot — lives
  in the closure's capture.
- A **catch** `cont` (built by `catch_cont`) waits on one slot and hands its
  terminal to a [`CatchFinish`](../src/machine/execute/outcome.rs) closure as a
  `Result<&KObject, KError>`. Unlike a combine, an errored dep does not
  short-circuit — the closure always runs and decides whether to recover or
  re-raise. The `TRY-WITH` builtin
  ([`try_with`](../src/builtins/try_with.rs); see
  [error-handling.md](error-handling.md)) is the sole caller today: it spawns its
  watched expression as a sub-dispatch and registers a catch that picks the
  matching branch by tag.
- A **decide** `cont` (built by `ignore_results`) takes no dep values — it reads
  the view and classifies / re-resolves — so its deps are park-only and the
  results slice is ignored.

## The dispatcher / scheduler boundary

The dispatch tree
([`execute/dispatch/`](../src/machine/execute/dispatch.rs)) is a sibling
of [`execute/scheduler/`](../src/machine/execute/scheduler.rs), not
nested inside it. Every scheduler-facing step — a dispatch decide, a
finish, a builtin body, an invoke — flows through one **decide → outcome →
apply** contract: it decides against a read-only view, *returns* the
scheduler mutations it wants as data, and a single harness method applies
them. The three pieces:

- **The read view** —
  [`SchedulerView<'run, 's>`](../src/machine/execute/dispatch/ctx.rs) wraps
  `&'s Scheduler<'run>` (never `&mut`). It exposes only the reads a decide
  needs: the static-over-the-step ones (`current_scope`, `chain_deref`,
  `active_chain`, `build_bare_outcomes`) and the live reads of
  *pre-existing* producers (`is_result_ready`, `would_create_cycle`,
  `read_result`). It permits scope binding (interior-mutable `&Scope`) but
  no graph write. The `DepGraph`, `NodeStore`, and active-frame fields stay
  `pub(in execute::scheduler)`; the dispatch shape modules (`keyworded`,
  `fn_value`, `single_poll`) never name scheduler fields directly. A future
  scheduler internal rename (`active_chain` → ..., `DepGraph` split) is a
  single-file change inside `scheduler/`.
- **The effect** —
  [`Outcome<'run>`](../src/machine/execute/outcome.rs) is the one currency
  every producer and finish returns (the dispatch-side peer of the builtin
  [`Action`](../src/machine/core/kfunction/action.rs)). It is AST-free — no
  variant names a `KFunction` or a `KExpression`. Four variants: `Done` (a
  value to lift, or an error), `Continue` (replace this slot's work and frame,
  re-run, no park), `ParkThenContinue` (park on deps, then run a
  [`Continuation`](../src/machine/execute/outcome.rs) that yields another
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
  [`KoanHarness<'run>`](../src/machine/execute/harness.rs) owns the `Scheduler`
  by composition (a `sched` field, not a `&mut` borrow) and is the **sole**
  holder of `&mut Scheduler` across the execute tree. Its
  [`apply_outcome`](../src/machine/execute/harness.rs) interprets a returned
  outcome into graph writes and the slot's `NodeStep`. Because only the harness
  reborrows the scheduler mutably, no decide handler holds `&mut Scheduler` —
  decide (against a read-only view) and apply (against `&mut self`) never
  overlap, and that separation is structurally enforced by the type rather than
  a naming convention. The execute loop, the AST-aware submission wrappers
  (`enter_block`, `dispatch_here`, `add_dispatch_in_frame`,
  `dispatch_body_statements`, `combine_here`), `submit_dispatch`, and the
  aggregate-literal lowering are all `&mut self` methods on `KoanHarness`. The
  unified node handler
  ([`run_wait`](../src/machine/execute/scheduler/finish.rs)) collects the slot's
  resolved dep terminals, builds a `SchedulerView`, runs the `cont` closure,
  reclaims the owned-dep suffix, and hands the outcome to `apply_outcome`.

`Scheduler`'s own surface is AST-free: it exposes read views plus the low-level
write primitives (`submit_node`, `alloc_slot`, `add_owned_edge` /
`add_park_edge`, `acquire_tail_frame`, `free`, `resolve_node_scope`,
`ensure_run_frame`, scope / chain reads) — no method signature names a
`KExpression` or an AST type. No trait wraps `Scheduler`: those graph-write
primitives are inherent methods capped `pub(in crate::machine::execute)`, so
only the harness reaches them. A builtin invoked mid-dispatch
(e.g. `newtype_construct`) routes through the shared
[`run_action`](../src/machine/execute/harness.rs) harness as a pure
`Action → Outcome` lowering; `exec::invoke` reads the dispatcher's ambient
`current_frame` / `current_lexical_chain` off the view to build the builtin's
`BodyCtx`.

## Callable result — the `Outcome` return shapes

A builtin or user-fn body, like every other step, returns an
[`Outcome`](../src/machine/execute/outcome.rs):

- `Done(Value)` — the body produced a final value; the slot finalizes.
- `Done(Err)` — structured failure; see [error-handling.md](error-handling.md).
- `Continue` — the body wants to dispatch a fresh expression in its own slot
  (TCO, see below); when the body has leading (non-tail) statements they
  become owned deps the slot parks on, and the `Continue` fires only from the
  resolving finish.

When a body cannot produce its result inline — its expression has nested
sub-expressions whose own evaluation hasn't run yet — the slot parks: its work is
rewritten to a `NodeWork` that waits on the spawned sub-dispatch deps and runs a
combine `cont` that assembles the result on wake. The slot keeps its index, so
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
[`Scheduler::finalize`](../src/machine/execute/scheduler/execute.rs)
method body that pairs `NodeStore::finalize` with `DepGraph::drain_notify`,
so the "every terminal write fires the notify" rule is type-enforced
rather than restated at each call site. Consumers arrive on the run-set
only when actually ready; there is no poll-and-requeue.

Every consumer wakes the same way: at pop time its `pending_deps` is zero, so
every dep is terminal, and [`run_wait`](../src/machine/execute/scheduler/finish.rs)
reads each resolved dep off the view by index and hands the `Result` slice to the
slot's `cont`. There is no per-edge wake-attribution side-channel — a decide that
re-resolves reads its producers from the rebuilt scope, not a wakes list.
`DepGraph::drain_notify` returns the per-consumer `hit_zero` flag so the
enqueue-on-zero runs off a single drain.

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

[`DepGraph`](../src/machine/execute/scheduler/dep_graph.rs) stores one
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
park-on-producer and a combine's `Existing` sibling parks — which `run_wait`
excludes from the reclaim by reading `deps[park_count..]` for the owned indices,
so clearing the owned tree cannot drop a wake intent on a sibling producer.

## Bare-name forward splice

The push/notify model assumes a single producer slot per result. A bare-name slot
(`(some_var)`, or the RHS of `LET y = z`) that resolves its name to a still-running
binding-producer would otherwise become a *second* producer of that result. Instead
the slot is **spliced out** as an alias of the producer, which stays the sole
producer. All the graph logic lives in
[`scheduler/splice.rs`](../src/machine/execute/scheduler/splice.rs):

- The bare-name decide returns [`Outcome::Forward(producer)`](../src/machine/execute/outcome.rs).
  If `producer` is already ready, the harness finalizes the slot with the
  producer's terminal directly ([`NodeStep::Done`](../src/machine/execute/nodes.rs)).
- Otherwise the slot's step yields [`NodeStep::Alias(producer)`](../src/machine/execute/nodes.rs),
  and the execute loop calls [`Scheduler::splice_forward`](../src/machine/execute/scheduler/splice.rs):
  the consumers already parked on the slot are moved onto the producer's notify
  list ([`DepGraph::splice_notify`](../src/machine/execute/scheduler/dep_graph.rs)),
  and the slot's [`SlotState`](../src/machine/execute/scheduler/node_store.rs)
  becomes `Aliased(producer)`. The aliased slot never fires; the producer's fire
  wakes the moved consumers directly.

Reads follow the alias to the real producer:
[`Scheduler::resolve_alias`](../src/machine/execute/scheduler/splice.rs) walks the
alias chain (iterative, always pointing downstream to a real producer, so it
terminates and never cycles), and `read_result` / `is_result_ready` resolve
through it. Edge installs resolve it too:
[`add_owned_edge`](../src/machine/execute/scheduler/splice.rs) /
[`add_park_edge`](../src/machine/execute/scheduler/splice.rs) wire a late consumer
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
is a `Continuation::Finish` — the dispatch flavor of a combine. The harness
submits each dep as a sub-Dispatch and parks the parent on a
[`NodeWork`](../src/machine/execute/nodes.rs) whose `cont` is a combine wrapping
that *splice finish* (a [`CombineFinish`](../src/machine/execute/outcome.rs)
closure). When the deps terminalize, that finish runs and writes each
resolved value back into the working copy:
`working_expr.parts[part_idx] = ExpressionPart::Future(value)`. The splice
lives **entirely inside the finish** — the scheduler resolves deps and hands
values back exactly as it does for any combine, learning nothing about `Future`
cells. The assembled `Future`-laden expression then goes through
`resolve_dispatch` as if it had been written with literals.

(This *expression* splice — rewriting `parts` to `Future` cells — is distinct
from the *slot* splice of [Bare-name forward splice](#bare-name-forward-splice),
which aliases one slot to another. They share the word but not the mechanism.)

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

An [`Action::Tail`](../src/machine/core/kfunction/action.rs), lowered to an
[`Outcome::Continue`](../src/machine/execute/outcome.rs) by `run_action`,
makes a tail return rewrite the **current scheduler slot's work** to a fresh
dispatch decide of `expr` and re-run in place — no new node allocated. Both deferring
builtins (`match_case`, and `run_user_fn` for user-fns) are tail by
construction. A chain of tail calls (`A → B → PRINT`, or unbounded
`LOOP → LOOP`) reuses one slot end-to-end. Verified by two slot-count
assertions in the test suite. When a body has leading (non-tail) statements,
they become owned deps the slot parks on (one body-block `DepRequest::BodyBlock`) and
the `Continue` tail fires only from the resolving finish — so the leading
siblings run, and cascade-free, before the tail-replace, restoring frame
uniqueness so [`try_reset_for_tail`](per-call-arena-protocol.md#tco-frame-reuse)
reuses the cart and TCO stays flat even for side-effecting multi-statement
bodies.

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
a sub-Dispatch that the parent slot parks on as an owned dep. Without
reclamation those slots accumulate per body iteration, so realistic recursive
code is O(n) scheduler memory even when its data footprint is O(1).

Reclamation runs in [`run_wait`](../src/machine/execute/scheduler/finish.rs) after
the `cont` closure returns its `Outcome`, before the harness applies it — so a
dispatch splice finish's freed indices are on the free-list before the harness
dispatches the spliced body. Once the consumer has read its dep results and either
spliced them into `working_expr.parts` as `Future(value)` (the eager-subs splice
finish) or handed them to its combine / catch finish, the owned dep
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
sub-struct on `Scheduler` that owns a single `slots` vector of `SlotState`
enums plus a `free_list: Vec<NodeId>` of recyclable indices. One enum encodes
the per-slot lifecycle — `PreRun(Node)` (an un-run node payload), `Running`
(payload moved out for its step), `Done(NodeOutput)` (terminal result),
`Aliased(NodeId)` (a bare-name forward spliced out to its producer), and `Free`
(reclaimed) — and each index moves through `alloc_slot → take_for_run →
reinstall* → finalize → free_one`. Each transition is a single atomic mutator
body, so the recycle-vs-extend choice, the take/reinstall pairing, the terminal
write, and reclamation are each encapsulated; because payload and result are the
same enum slot, no call site outside `NodeStore` can land a `Done` without the
node having been taken, nor read a result before it is `Done`.
Dependency bookkeeping lives alongside it in a single
[`DepGraph`](../src/machine/execute/scheduler/dep_graph.rs) sub-struct
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
[`splice.rs`](../src/machine/execute/scheduler/splice.rs)) resolve the
producer through any alias and short-circuit a producer that is already
terminal; `splice_notify` moves a spliced-out slot's dependents onto its
producer's row.
`Scheduler::add` orchestrates across the two sub-structs: `NodeStore::alloc_slot`
picks the index (popping `free_list` or extending) and `DepGraph::install_for_slot`
branches privately on whether the slot is recycled or freshly extended to
write the dep entries in lockstep. See also
[memory-model.md § Performance notes](memory-model.md).

A known limitation: each top-level dispatch retains a small constant of
persistent slots — the entry slot returned to the user, and, for a bare-name
binding (`LET y = z`), the spliced-out alias slot plus its producer. An aliased
slot is never freed (it has no parent to reclaim it), and a top-level producer
has no parent either. So each `add_dispatch` costs a small constant rather than
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

### Miri forward-splice and replay-park lifetime contract

A bare-name slot whose name resolves to a still-running producer is spliced out
as an alias of it (see [Bare-name forward splice](#bare-name-forward-splice)). A
read of the aliased slot resolves to the producer and returns the producer's own
`&KObject<'a>` reference — not a clone. The producer's arena therefore must
outlive every consumer that reads through the alias. The replay-park route is
symmetric: a parked dispatch decide's captured scope, and the `&KObject<'a>` its
resolved producers carry, must stay valid across the wake and the re-dispatch.
The `lift_park_minimal_program_for_miri` (a bare-name forward, `LET y = z`) and
`replay_park_minimal_program_for_miri` (a parked-and-resumed FN call) tests pin
the contract under Miri tree borrows.

### Submission-time binder install and recursive sub-Dispatch

The dispatch-layer submission chokepoint
[`dispatch::submit_dispatch`](../src/machine/execute/dispatch/submit.rs)
inspects every dispatch submission against the dispatching scope's ancestor
chain via `extract_binder_install`: it finds the first overload in the
matching `functions[expr.untyped_key()]` bucket whose `binder_name` OR
`binder_bucket` extractor returns `Some(_)` for the expression. The picked
overload's install channel is reified as `BinderKey::Name(name)` (for `LET` /
`STRUCT` / `UNION` / `SIG` / `MODULE`) or `BinderKey::Bucket(key)` (for `FN` /
`FUNCTOR`); the install site stamps the corresponding `placeholders[name]` or
`pending_overloads[bucket]` entry on the dispatching scope before the slot is
ever popped from the work queues. A later sibling that dispatches before the
binder's slot pops finds the entry and parks rather than surfacing
`UnboundName` / `DispatchFailed`. The binder logic lives in the dispatch layer,
not the scheduler: the scheduler exposes only a generic slot allocator
(`Scheduler::submit_node`) and the `Scope::install_*` primitives, so no
`NodeWork` variant or scheduler code names a `KExpression`.

For binder-shaped expressions, `submit_dispatch` also recurses into the eager
Expression-shaped argument slots and submits each as a sub-dispatch *at the same
outermost submission point*. The walk computes an `eager_slot_mask` over the
bucket — a slot is eager only if *every* binder overload in the bucket marks it
non-`KType::KExpression`; any overload tagging a slot lazy keeps that slot out
of the recursive walk because the eventual dispatch may resolve to that
overload. Lazy slots — FN body, FN signature/return-type-`KExpression` overload,
FUNCTOR body, MODULE body — dispatch in the callee's scope at body-invoke time,
not here. Each recursive `submit_dispatch` runs its own
`extract_binder_install`, so a nested binder's placeholder installs at the same
outermost step as its parent's; recursion terminates at non-binder leaves and at
lazy slots, bounded by AST depth.

The collected `(slot_idx, sub_node_id)` pairs are captured (with `expr`) in the
parent's birth dispatch decide closure
([`decide_with_presubs`](../src/machine/execute/dispatch.rs)). When the parent runs,
the fused splice / park / eager-sub walk in
[`dispatch.rs`](../src/machine/execute/dispatch.rs) consults
`pre_subs` before the `Expression` / `ListLiteral` / `DictLiteral` arms:
a slot already pre-submitted reuses the existing `NodeId` (and replaces
the part with an empty-`Identifier` placeholder for the eventual expression
splice) rather than allocating a fresh sub-Dispatch. The
`KeywordedState::install_bare_name_park` and `install_overload_park`
installers carry `pre_subs` into the `KeywordedState.init.pre_subs`
field of the parked state, and `KeywordedState::resume` hands it back to
`initial` on wake — so a park-and-wake cycle does
not re-allocate the pre-submitted children.

Statement indices are per-`enter_block` call: each call to
[`KoanHarness::enter_block`](../src/machine/execute/scheduler.rs) mints
chain frames at indices `1..N` for the N statements it submits. A REPL
or test fixture that submits without an ambient chain (the
[`Scheduler::add`](../src/machine/execute/scheduler/submit.rs) auto-root
branch) gets [`LexicalFrame::detached`](../src/machine/core/lexical_frame.rs)
— a chain that mentions no real scope, so the visibility predicate's
`index_for → None ⇒ complete` arm makes every binding in the target
scope visible. This is what lets a REPL query read through to every
prior bind without sharing an index space with them.

The execute side — [`classify_dispatch`](../src/machine/execute/dispatch.rs) —
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
eager-shaped part as a `Combine` dep and parks this slot on them;
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
    `Value` returns a `Done` outcome inline, `Placeholder` returns
    `Outcome::Forward(producer)`, whose harness splices the slot out as an alias
    of that producer (see [Bare-name forward splice](#bare-name-forward-splice)),
    `UnboundName` falls through to the keyworded path so `value_lookup`'s body
    produces the structured error.
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
  - `SigiledTypeExpr` (single-part `:(...)` wrapper) — the `classify_dispatch`
    arm tail-replaces the slot with a fresh `Decide`
    of the wrapped `KExpression`, so the inner expression runs through the
    same classifier and produces the same carrier shape any other dispatch
    site does. See
    [type-language-via-dispatch.md](typing/type-language-via-dispatch.md)
    for the full type-language dispatch contract.
  - `RecordType` (single-part `:{…}` record type) — `record_type` folds the
    field list straight to `KType::Record` through the shared field-list
    elaborator (no tail-replace, no internal type-constructor builtin),
    deferring through a combine `cont` only when a field type forward-references
    or sub-dispatches. See
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
  dispatch poll, shared across admission and the walk.
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
    sub-Dispatch (the sub-Dispatch enters `classify_dispatch`'s matching shape arm —
    `SigiledTypeExpr` tail-replaces with the inner dispatch, `RecordType` folds
    to `KType::Record`); `ListLiteral` and `DictLiteral`
    route through `schedule_list_literal` / `schedule_dict_literal` for the
    aggregate Combine; any other shape rides through unchanged. Lazy
    `Expression` parts in `KExpression` slots are filtered out by
    `eager_indices` and the receiving builtin dispatches them itself.

  **Park-precedence guard.** Sub-Dispatch and aggregate scheduling are
  staged into a `PendingSub` vec rather than submitted eagerly during the
  walk. After the loop, if `producers_to_wait` is non-empty the decide
  returns a `ParkThenContinue` whose continuation is a `Continuation::Resume`
  (carrying a `ResumeFn` closure over the partly-spliced `working_expr`) — the
  harness installs the park edges as `Notify` (via `add_park_edge`) and
  installs a resume dispatch decide, so the captured
  `working_expr` becomes the source of truth on wake — **without** submitting
  any staged subs. Eager submission would leak the sub-nodes on the re-resume
  wake path, where the closure would re-stage them. Multi-name forward
  references compose as one combined park rather than N independent
  sub-Dispatches.

  If no producer parked, the driver applies each `PendingSub`: `Reuse(id)`
  for slots already pre-submitted recursively at outermost-submission time
  (see [Submission-time binder install and recursive
  sub-Dispatch](#submission-time-binder-install-and-recursive-sub-dispatch)),
  `Dispatch(sub_expr)` for a fresh sub-Dispatch, and `ListLit` / `DictLit`
  for the aggregate. With no subs to schedule the driver binds the picked
  function directly: the decide folds the resolved call into a dep-free
  `Outcome::Continue` (via `dispatch::exec::invoke_continue`) whose frame
  placement installs the per-call cart and whose `work` re-decides via
  `dispatch::exec::invoke` on the next pop
  (a wrap-slot-only call like `MAKESET IntOrd` resolves bare names in Step 4,
  leaves no eager parts, and binds in one step — no Combine detour). Otherwise
  the decide returns a `ParkThenContinue` with a `Continuation::Finish`
  declaring the fresh subs as deps with a splice finish; the harness parks the
  slot as a `Combine` carrying the finish. At dep completion the finish
  re-resolves the spliced `working_expr` and folds it into a `Continue` — via
  `invoke_continue` on the speculatively-picked function, or via
  `redispatch_continue` (re-running
  [`keyworded::finish`](../src/machine/execute/dispatch/keyworded.rs)) when
  none was pre-picked.

  Dict and list literals (`classify_aggregate_part` in
  [`dispatch/literal.rs`](../src/machine/execute/dispatch/literal.rs))
  ride the same name-resolve rail when their `wrap_identifiers` plan-input
  is set: bare-name entries call `resolve_name_part` directly and
  materialize as `Slot::Static` (resolved) or `Slot::Park(i)` (parked
  producer), with the combine driving a single wake across all parked
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

A bare-identifier slot resolving to a producer returns `Outcome::Forward` and is
spliced out (above). The other parking fast-lane handlers (the `fn_value`
`FunctionValueCall` head-placeholder park) and the eager-resolve pass return a
`ParkThenContinue` with a `Continuation::Resume` for a re-resolve, whose harness
calls `DepGraph::add_park_edge` — recording a `DepEdge::Notify(producer)` in the
consumer's `dep_edges` entry alongside the `DepEdge::Owned(child)`
entries that mark sub-slots the consumer owns. The bare-name splice likewise wires
the moved consumers through `add_park_edge` against the resolved producer. `add_park_edge` and its
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
schedules a combine over those producers that re-runs the signature
elaboration against the now-final scope at finish time. (See
[typing/elaboration.md § Layers](typing/elaboration.md#layers) § Layer 3
for the elaborator's role in the pipeline.) A parens-wrapped
parameter type (`xs :(LIST OF Number)`) rides the same combine:
`parse_fn_param_list` records the `(slot_idx, sub_expr)` pair, FN-def
schedules each sub-expression as its own sub-Dispatch, and the combine's
finish closure splices each result into
`signature_expr.parts[slot_idx]` as `Future(Carried::Type(_))` before
re-running the parameter-list walk against the spliced signature. STRUCT
and UNION share the same elaborator-and-combine shape for their
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
the top-level result read panic on an unresolved slot. `sample` is the carrier
summary of the first parked node that has one (a dispatch decide carries its
expression's pre-rendered summary; a carrier-less combine/catch wait falls back
to a generic tag), so the diagnostic points at code the reader can act on.

### Dispatch birth and resume

A dispatch slot is the one [`NodeWork`](../src/machine/execute/nodes.rs) shape with
a decide `cont` (built by [`ignore_results`](../src/machine/execute/outcome.rs))
and a `carrier` deadlock-summary string. The `cont` captures a
`SchedulerView -> Outcome` closure that reads the view, classifies / re-resolves,
and returns an `Outcome`; it takes no dep values, so its deps are park-only. Birth
and resume are the same shape, run through the same handler
([`run_wait`](../src/machine/execute/scheduler/finish.rs)); the scheduler never
switches on dispatch-internal state and `NodeWork` names no `KExpression`.

**Birth** closures are built by the dispatch layer
([`decide`](../src/machine/execute/dispatch.rs) / `submit_dispatch`) capturing the
slot's `expr` (+ `pre_subs`). On first poll the closure runs `classify_dispatch`,
which classifies `expr` via `classify_dispatch_shape` and decides against a
`SchedulerView`, returning an `Outcome`. `pre_subs` carries any recursively
pre-submitted sub-Dispatches keyed by their slot index in `expr.parts`, populated
at submit time for binder-shaped expressions so a nested binder's placeholders
install at the outermost submission point; `classify_dispatch` reuses these instead
of allocating fresh sub-Dispatches.

When a decide must wait — a keyworded resolve that found bare-name or
overload producers, a `FunctionValueCall` head still resolving to a
`Placeholder`, a `TypeCall` parked on a still-finalizing head — it returns a
`ParkThenContinue` whose continuation is a `Continuation::Resume` carrying an
opaque [`ResumeFn`](../src/machine/execute/dispatch.rs) closure
(`SchedulerView -> Outcome`, built by `park_resume`). The harness parks the
slot's edges and installs a fresh **resume** decide carrying that closure. On
wake, `run_wait` clears the slot's stale dep edges, runs the captured closure
against a fresh `SchedulerView`, and applies its `Outcome` — **one uniform arm**
for every shape. Clearing on resume is uniform and safe: a dispatch park installs
only `Notify` edges (sibling forward references, never children), which drop at
free, so a resume re-deriving its producers from the rebuilt scope cannot drop a
live wake. (Clearing on a fresh birth is a no-op — it owns no dep edges yet.)

Each family's closure captures exactly what its decide needs and re-runs it
against the now-populated scope:

- A **keyworded** bare-name park re-enters against the carried (partly-spliced)
  `working_expr`; the bare names now resolve through `scope.resolve_with_chain`
  to bound values, so the rebuilt `bare_outcomes` picks them up and the
  wrap-slot splice fires `Future(obj)` on the second pass.
- A keyworded **overload** park carries the original (unspliced) expression and
  re-runs the resolve against the now-populated `pending_overloads` bucket.
  **Eager subs never park here**: a `Deferred`/eager-subs resolve returns a
  `ParkThenContinue` with a `Continuation::Finish` and parks on a node with a
  combine `cont` whose finish re-resolves the spliced expression — so a
  keyworded resume never re-enters for them. Re-resolve in the finish is
  authoritative: an element-typed `Future(_)` that narrows a typed-slot
  admission rules a speculative initial pick out, and the call surfaces
  `DispatchFailed` (non-match) rather than committing to a bind-time
  `TypeMismatch`.
- A **`FunctionValueCall`** head-placeholder park (`fn_value::install_head_park`)
  carries the original call expression and re-runs the fast lane once
  `scope.resolve_with_chain` lands in the `Resolution::Value` arm. Its eager
  subs route through `apply_callable::install_eager_subs_track`, which returns
  a `Continuation::Finish` carrying the picked `KFunction` from the head directly;
  `FunctionValueCall` is non-overload-set, so a typed `Future(_)` an eager sub
  reveals can't narrow the pick and the finish binds `picked` without
  re-resolving.

**Park exclusivity holds by construction.** A single resolve reaches exactly
one park installer: the overload park installs from a resolve failure *before*
the part walk runs; the bare-name park installs *before* any eager sub could
stage, because the part walk's park-precedence guard runs first; eager subs
take the combine-finish route rather than a resume. So a slot's resume
carries exactly one park reason.

The drain-end cycle-detection guard (`NodeStore::unresolved`) summarizes parked
slots from each `NodeWork`'s `carrier` — a dispatch decide carries its
expression's pre-rendered summary; a carrier-less combine/catch wait falls back to
a generic `<wait>` tag — selected by a testable `work_deadlock_sample` helper in
`node_store`.

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
  parked on it through a combine or catch `cont`) but unbounded
  in principle (forward-reference parks, where the splice moves many
  consumers onto one producer).

### What amortizes

- **Slot recycling.** `Scheduler::reclaim_deps` frees sub-slots eagerly
  during [`run_wait`](../src/machine/execute/scheduler/finish.rs), and `add()`
  pulls
  from the free-list before extending the underlying vectors. A
  steady-state recursive body reuses the same slot indices across
  iterations; `body_subexpression_slots_recycle_across_calls` pins the
  bound at ≤3 net slots/call.
- **Tail-call slot rewrite.** An `Action::Tail` (lowered to
  `Outcome::Continue`) rewrites the current slot's work in place rather than
  allocating a new one — one slot for an arbitrarily deep tail-call chain.
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

### Single entry point: `KoanHarness::enter_block`

Every dispatched node has a chain because every new lexical block is
entered through one primitive. `KoanHarness::enter_block(scope_id,
statements, scope)` prepends a frame `(scope_id, i)` for each
statement `i` onto the current ambient chain and submits the
statements as dispatch nodes:

- Top-level statements
  ([`interpret`](../src/machine/execute/interpret.rs)) enter through
  `enter_block(root.id, exprs, root)` against an empty parent chain.
- `MODULE` and `SIG` bodies enter through the dispatch harness's `InScope`
  fan-out
  ([`apply_outcome`](../src/machine/execute/harness.rs)), which splits
  via the shared
  [`split_body_statements`](../src/machine/core/kfunction/body.rs) helper and
  submits each statement through `enter_block`. The scheduler itself never
  inspects AST shape — `split_body_statements` is the single source of truth for
  the split.
- FN, FUNCTOR, MATCH-arm, and TRY-arm bodies split via that same
  [`split_body_statements`](../src/machine/core/kfunction/body.rs) helper
  (the all-`Expression` rule): the body's
  non-tail statements ride along as the `leading` field of an
  [`Action::Tail`](../src/machine/core/kfunction/action.rs), and the slot
  parks on them as owned deps before tail-replacing into the last statement.
  Its `block_entry` names the body/arm scope; the harness derives the chain
  indices and the tail's `body_index` from `block_entry` + `leading`. TCO is
  preserved on the last statement. Single-statement bodies carry empty
  `leading` and tail-replace directly.
- FN bodies route through `run_user_fn` (see below — the chain
  shape is special because the call site's chain is not the body's
  lexical chain).

The "every dispatched node has a chain" invariant is an `expect` in
[`Scheduler::submit_node`](../src/machine/execute/scheduler/submit.rs); the
public `add_dispatch` entry auto-roots a chain when no ambient one is present
via [`LexicalFrame::detached`](../src/machine/core/lexical_frame.rs) (so
REPL-style submissions outside `enter_block` see every prior bind in the target
scope).

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
through the `Action::Tail { leading, block_entry }` shape (see
[Single entry point: `KoanHarness::enter_block`](#single-entry-point-koanharnessenter_block)
above).

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
  dispatch-decide and combine machinery — type-returning builtins on the value
  path, a combine `cont` as the refinement-and-wake-up mechanism, and stage 5
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
