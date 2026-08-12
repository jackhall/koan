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
  slots, short-circuits on the first errored dep, and otherwise hands the
  resolved dep terminals (un-relocated: each terminal *is* the producer's
  lifetime-free delivery envelope, read under its own pins) to a single
  [`TerminalDepFinish`](../../src/machine/execute/outcome.rs) closure — the one
  delivery currency. A value-reading finish writes that shape directly; a value
  that must outlive the resolving step travels as its delivery envelope, adopted at
  the consumer's own step brand — every delivery, including the catch channel,
  is carrier-only; no dep ever crosses to a finish as a relocated copy. A
  [`WitnessedDepFinish`](../../src/machine/execute/outcome.rs) (folds terminals
  into one witnessed carrier) projects onto the same currency through
  `seal_witnessed` before `short_circuit` ever sees it, so there is exactly one
  delivery loop. List- and dict-literal planners use the witnessed shape; the
  construction logic — including already-resolved literal scalars that don't need
  a dep slot — lives in the closure's capture.
- A **catch** `cont` (built by `catch_cont`) waits on one slot and hands its
  terminal to a [`CatchFinish`](../../src/machine/execute/outcome.rs) closure as a
  `Result<Sealed<CarriedFamily, FrameSet>, KError>` — the watched producer's own
  sealed carrier, duplicated with its pins. Unlike a dep-finish, an errored dep does not
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

The scheduler ([`scheduler`](../../workgraph/src/scheduler.rs)) is a crate-root sibling of
`machine`, not nested inside it: a workload-independent DAG of dependency-linked
nodes, generic over a [`Workload`](../../workgraph/src/scheduler/workload.rs) and naming no Koan
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
  `build_bare_outcomes`) and the live cycle guard on *pre-existing* producers
  (`would_create_cycle`) — readiness is not a view read: install returns
  filled-or-parked when the harness applies the outcome. It permits scope
  binding (interior-mutable `&Scope`) but no graph write. The scheduler's `queues`
  / `deps` / `store` fields stay `pub(in crate::scheduler)`; the dispatch shape
  modules (`keyworded`, `fn_value`, `single_poll`) never name scheduler fields
  directly.
- **The effect** —
  [`Outcome<'step>`](../../src/machine/execute/outcome.rs) is the one currency
  every producer and finish returns (the dispatch-side peer of the builtin
  [`Action`](../../src/machine/core/kfunction/action.rs)). It is AST-free — no
  variant names a `KFunction` or a `KExpression`. Its single lifetime `'step` is the
  per-step cart-scale frame lifetime the `Done` value is born at — the `Done` carrier rides it as a
  [`StepCarried`](../../src/machine/execute/step_carried.rs), confined to the step until it exits
  through `seal_at_step` into finalize; the delivery walk adopts it
  across each dep edge ([per-call-region/lifecycle.md § Node-output delivery](../per-call-region/lifecycle.md#node-output-delivery)).
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
  executing slot's lexical payload (scope handle + chain, projected from the
  slot's anchor), and the slot's declared-return obligation
  (the continuation capture a tail chain carries; its presence *is* the
  contract-chain flag) — lives on the
  driver ([`ambient`](../../src/machine/execute/ambient.rs)), not the scheduler,
  which is a pure DAG runtime. Its
  [`apply_outcome`](../../src/machine/execute/runtime.rs) interprets a returned
  outcome into graph writes and the slot's `NodeStep`. Because only the harness
  reborrows the scheduler mutably, no decide handler holds `&mut Scheduler` —
  decide (against a read-only view) and apply (against `&mut self`) never
  overlap, and that separation is structurally enforced by the type rather than
  a naming convention. The execute loop, the AST-aware submission wrappers
  (`enter_block`, `dispatch_in_own_scope`, `dispatch_in_active_frame`,
  `dispatch_body`, `submit_dep_finish_in_own_scope`), `submit_expression`, and the
  aggregate-literal lowering are all `&mut self` methods on `KoanRuntime`. The
  unified node handler
  ([`run_step`](../../src/machine/execute/run_loop.rs)) receives the slot's
  deps as residents of its region — delivered at each producer's finalize, so
  step start is zero graph work — builds a `SchedulerView`, runs the `cont`
  closure, and hands the outcome to `apply_outcome`.

The scheduler reaches the driver only through its method contract, and every
method names only `EdgeId` and the workload's associated types — no signature
names a `KExpression`, `Scope`, or AST type, and node identities stay
library-internal. `pop_next` / `take_for_run` / `replace` drive a slot's
lifecycle; `alloc_node`, `install_edges` and `splice_forward` wire the graph —
`install_edges` being the single door for wiring an already-allocated slot,
routing the same scheduler-internal wire primitive `alloc_node` uses, and
install returning filled-or-parked per edge; `finalize` delivers and reclaims;
`would_create_cycle` / `unresolved` are the reads. No trait
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

## Which edges Koan installs

The edge mechanism — producer → consumer notify lists, per-consumer pending
counters, the two-band run set, and the three row invariants that keep them
coherent — is the library's
([workgraph/design/dag-scheduler.md](../../workgraph/design/dag-scheduler.md)).
What is Koan's is *which* edges each dispatch shape installs:

- **Owned** deps are the sub-Dispatches a slot spawns for its own nested
  sub-expressions — the deps of the working-copy splice, a body block's leading
  statements. Their edges name the spawning consumer's region as destination.
- **Park** deps are Koan's wait-on-someone-else's-producer cases: a dispatch
  decide's park-on-producer when a name resolves to a still-running binding
  producer, and a dep-finish's `Existing` sibling parks. A placeholder park
  names the *original* destination scope's region on its edge — delivery dedups
  per distinct destination, so the eventual binding write and the placeholder
  share one adopt.

To the scheduler both are the same edge; the owned/park split is Koan's `Deps`
currency labeling — positional addressing plus dispatch classification, which
is install-and-inspect: install returns filled-or-parked, and the
already-errored and would-cycle arms are Koan policy read off the installed
edge ([dispatch.rs](../../src/machine/execute/dispatch.rs)).

Every consumer wakes the same way: at pop time its pending count is zero, so every
dep is terminal, and [`run_step`](../../src/machine/execute/run_loop.rs) reads each
resolved dep off the view by index and hands the `Result` slice to the slot's
`cont`. There is no per-edge wake-attribution side channel — a decide that
re-resolves reads its producers from the rebuilt scope, not a wakes list.

Koan's own use of the two priority bands is thin: top-level `dispatch_in_scope`
calls route through the top-level band so independent top-level expressions execute
in submission order, and everything else — wakeups, Replace-arm re-enqueues,
ready-on-arrival nodes — is internal work that drains first.

## Bare-name forward splice

The push/notify model assumes a single producer slot per result. A bare-name slot
(`(some_var)`, or the RHS of `LET y = z`) that resolves its name to a still-running
binding-producer would otherwise become a *second* producer of that result. Instead
the slot is **spliced out** as an alias of the producer, which stays the sole
producer.

Koan's half is the trigger: the bare-name decide returns
[`Outcome::Forward(producer)`](../../src/machine/execute/outcome.rs). If `producer`
is already ready, the harness finalizes the slot with the producer's terminal
directly ([`NodeStep::Done`](../../src/machine/execute/nodes.rs)); otherwise the
slot's step yields [`NodeStep::Alias(producer)`](../../src/machine/execute/nodes.rs)
and the library takes over — re-pointing the slot's parked edges at the
producer once, so no aliased slot survives as a residual and no alias chain is
walked on reads
([workgraph/design/dag-scheduler.md § Alias splice](../../workgraph/design/dag-scheduler.md#alias-splice)).
Nothing in Koan has to be alias-aware.

## Working-copy splice

The scheduler dispatches each expression through its own
[`WorkingExpression`](../../src/machine/model/ast/working.rs) — a **working copy**
whose parts run lives in the dispatching step's region, distinct from the raw AST
type so that no value can ever hold one
([expressions-and-parsing.md § Two nodes](../expressions-and-parsing.md#two-nodes-raw-ast-and-the-schedulers-working-copy)).
The keyworded dispatcher extracts every nested sub-expression out of
the parent's `parts` (replacing each with a placeholder `StagedSlot`) and
declares them as the deps of a
[`ParkThenContinue`](#the-dispatcher--scheduler-boundary) whose continuation
is a `Continuation::FinishTerminal` — the dispatch flavor of a dep-finish. The
harness submits each dep as a sub-Dispatch and parks the parent on a
[`NodeWork`](../../src/machine/execute/nodes.rs) whose `cont` is a dep-finish wrapping
that *splice finish* (a [`TerminalDepFinish`](../../src/machine/execute/outcome.rs)
closure). When the deps terminalize, that finish rests each resolved value's
delivery envelope in the finishing step's own region and freezes a new parts run
with each staging hole replaced by its resting cell —
`WorkingPart::Spliced { cell }` — through
[`WorkingExpression::respliced`](../../src/machine/model/ast/working.rs). Every
part is `Copy`, so that is a memcpy with the holes patched, not a rebuild. The splice
lives **entirely inside the finish** — the scheduler resolves deps and hands
values back exactly as it does for any dep-finish, learning nothing about
`Spliced` cells. The assembled `Spliced`-laden expression then goes through
`resolve_dispatch` as if it had been written with literals.

(This *expression* splice — rewriting `parts` to `Spliced` cells — is distinct
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
siblings run, and are reclaimed, before the tail-replace, so the tail hop
[reinstalls the slot](../tail-call-optimization.md#the-design-reinstall-the-slot-turn-over-the-region)
cleanly and TCO stays flat even for side-effecting multi-statement bodies.

Because the reinstall applies after a step returns (never mid-step), the
retiring incarnation's region is past every borrow into it by the time it is
retired — the run-then-apply ordering supplies the safety, so a tail hop needs
no in-place frame reset. See
[tail-call-optimization.md § Soundness](../tail-call-optimization.md#soundness).

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

Reclamation is delivery itself
([workgraph/design/dag-scheduler.md § Delivery at finalize](../../workgraph/design/dag-scheduler.md#delivery-at-finalize)):
each sub-Dispatch's slot reclaims at its own finalize, the moment its notify
drains — its value is already a resident of the parent's region by then, so the
slot has nothing left to hold. A dispatch splice finish's dep indices are on
the free list before the harness dispatches the spliced body, with no separate
release pass in [`run_step`](../../src/machine/execute/run_loop.rs); the
consumer's teardown releases only the `EdgeId`s it still holds.

The net effect: recursive bodies whose only persistent state is the call result run
in O(1) scheduler memory across iterations, with the per-iteration fanout (the
body's transient sub-Dispatches) recycled through the library's free list. See also
[memory-model.md § Performance notes](../memory-model.md).

Per-top-level-dispatch persistent slots (the entry slot returned to the user,
and for a bare-name binding the spliced-out alias slot plus its producer) are
run-rooted and reclaim at the drain boundary — linear in *live* call count,
never multiplicative in body size.

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

