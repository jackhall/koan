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

## Working-copy splice

The scheduler dispatches each expression by mutating an **owned working
copy** of it. `run_dispatch` extracts every nested sub-expression out of
the parent's `parts` (replacing each with a placeholder `Identifier`),
spawns it as a sub-Dispatch, and parks the parent as
`NodeWork::Bind { expr: rewritten_expr, subs }`. When the subs terminalize,
`run_bind` writes each result back into the parent: `expr.parts[part_idx]
= ExpressionPart::Future(value)`. The assembled `Future`-laden expression
then goes through `resolve_dispatch` as if it had been written with
literals.

Source-of-truth ASTs are never mutated. The working copy is cloned from
its source at slot-submission time — `KFunction::invoke` clones the FN
body, `match_case::body` and `try_with` clone their picked arm, top-level
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
builtins (`match_case`, `KFunction::invoke` for user-fns) are tail by
construction. A chain of tail calls (`A → B → PRINT`, or unbounded
`LOOP → LOOP`) reuses one slot end-to-end. Verified by two slot-count
assertions in the test suite.

The slot's `Rc<CallArena>` is held in exactly one place during each step:
[`Scheduler::execute`](../src/machine/execute/scheduler/execute.rs) moves
`node.frame` directly into `self.active_frame` (no clone) and reverses the
move after the step. That single-ownership discipline is what lets the
tail-reuse path detect "nothing escaped" via `Rc::strong_count == 1`:
[`SchedulerHandle::try_take_reusable_frame_for_tail`](../src/machine/core/kfunction/scheduler_handle.rs)
takes the active frame, refuses to hand it out if any clone exists, and
otherwise lets `KFunction::invoke` reset the frame in place via
[`CallArena::try_reset_for_tail`](../src/machine/core/arena.rs) — swap the
inner `RuntimeArena` for a fresh empty one, re-allocate the child `Scope`,
re-link `outer` to the new call's captured scope. The shell, the heap-pinned
arena address, and the slot's `frame` field all survive across the
iteration; only the storage turns over. Frames carrying an escaped closure
(or any other clone of the `Rc`) fall through to a fresh `CallArena::new`,
preserving snapshot semantics for the escaped value.

A subtle point: host-stack overflow on naïve recursion is solved by the graph
model itself, not by `Tail`. Every "recursive call" enters the scheduler's
run-set rather than growing the Rust call stack — that property is
structural, not optimizing. What `Tail` adds is constant **scheduler-vec**
memory across the tail-call chain; frame reuse on top of it keeps **heap
memory** constant too.

## Transient-node reclamation

`Tail` reuses the outermost slot but bodies typically have internal
sub-expressions — the predicate of an `IF`/`MATCH` guard, the argument
expressions of a recursive call, list/dict literal elements. Each spawns a
sub-`Dispatch` and a parent `Bind`/`Combine` slot. Without reclamation those
slots accumulate per body iteration, so realistic recursive code is O(n)
scheduler memory even when its data footprint is O(1).

Reclamation runs at the end of `run_bind` / `run_combine`. Once a Bind has
read its dep results and spliced them into `expr.parts` as `Future(value)`
(or a Combine's finish closure has produced its result), the dep slots are
unreachable: a sub-Dispatch is owned by exactly one Bind / Combine, recorded
in the consumer's `dep_edges` entry as a `DepEdge::Owned(NodeId)`.
Free walks recursively, recycling each dep's own dep tree, and stops at any
still-live slot via `NodeStore::is_live` — so a free that dives into another
in-flight user-fn call leaves that subtree for that call's own reclamation.

The net effect: recursive bodies whose only persistent state is the call
result run in O(1) scheduler memory across iterations, with the per-iteration
fanout (the body's transient sub-Dispatches/Binds) recycled through a
free-list of slot indices that `add()` pulls from before extending the vecs.
Slot-table state lives in a
[`NodeStore`](../src/machine/execute/scheduler/node_store.rs)
sub-struct on `Scheduler` that owns three private vectors — `nodes:
Vec<Option<Node<'a>>>` (active node payloads), `results:
Vec<Option<NodeOutput<'a>>>` (terminal results), and `free_list: Vec<usize>`
(recyclable indices) — and the slot lifecycle that moves each index through
them: `alloc_slot → take_for_run → reinstall* → finalize → free_one`. Each
transition is a single atomic mutator body, so the recycle-vs-extend choice,
the take/reinstall pairing, the terminal write, and reclamation are each
encapsulated; no call site outside `NodeStore` can grow `nodes` without
`results` or land a `NodeOutput` without firing the notify-walk.
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
`UnboundName`. The park is keyed off `Scope::resolve` consulting the
`placeholders` table, so it covers every name reached through that path —
bare-name value slots and type-token slots. A *keyword-headed* function call
(`ID 7`) is the exception: it resolves through the `functions` bucket, which
does not consult `placeholders`, so calling a function not yet registered in
the same scope fails rather than parking (forward calls from a function body
are unaffected — bodies re-dispatch per call, after every sibling has
registered). The mechanism lives in two pieces.

A `placeholders` table — a `RefCell<HashMap<String, NodeId>>` — lives
inside the [`Bindings`](../src/machine/core/scope.rs) façade on
`Scope`, alongside `data` and `functions`. When a binder dispatches, its
`pre_run` hook (a per-`KFunction` extractor that pulls the to-be-bound name
structurally out of the expression's parts) installs `name → producer NodeId`
in the dispatching scope's placeholders. The six binder builtins (`LET`,
`FN`, `STRUCT`, `SIG`, `UNION`, `MODULE`) opt in via
`register_builtin_with_pre_run`; everything else stays placeholder-free.
`Scope::resolve` walks `data` then `placeholders` in each scope on the chain
and returns one of three shapes: `Resolution::Value(&KObject)` for a
finalized binding, `Resolution::Placeholder(NodeId)` for a still-running
producer, or `Resolution::Unbound` for a genuinely missing name. `bind_value`
and `register_function` remove their own placeholder before inserting into
`data` / `functions`, so the two tables are mutually exclusive at any
moment.

The execute side — [`run_dispatch`](../src/machine/execute/scheduler/dispatch.rs) — is a
five-phase linear pipeline: a bare-name short-circuit, the chain-walked
resolution, the placeholder install, the auto-wrap + replay-park rewrite,
and the dep schedule. Phase 2 calls
[`Scope::resolve_dispatch`](../src/machine/core/scope.rs) once and
matches on its [`ResolveOutcome`](../src/machine/core/scope.rs):
`Resolved(r)` continues into phase 3 with the picked function plus the
per-slot index buckets `r.slots` carries (`wrap_indices`, `ref_name_indices`,
`eager_indices`); `Ambiguous(n)` surfaces as an `AmbiguousDispatch` error;
`Unmatched` surfaces as `DispatchFailed`; `Deferred` (no match against
the bare shape but the expression carries nested `Expression` /
`ListLiteral` / `DictLiteral` parts whose evaluation may produce typed
`Future(_)` parts that match) jumps to phase 5's eager-fallthrough loop and
re-dispatches via [`run_bind`](../src/machine/execute/scheduler/finish.rs)
after subs resolve.

The four rails the resolution feeds:

- **Bare-name short-circuit** (phase 1, runs before resolution). A
  single-`Identifier` dispatch slot (`(some_var)`) consults `Scope::resolve`
  directly: `Value` returns inline, `Placeholder` rewrites the slot's work
  to `Lift(LiftState::Pending(producer_id))` (the same shim `BodyResult::Tail` uses for
  sub-Bind waits), `Unbound` falls through so `value_lookup`'s body
  produces the structured error.

  Forward references resolve through this rail and the auto-wrap rail
  (below), both of which route name lookups through `Scope::resolve` and so
  consult the `placeholders` table. A *keyword-headed* call — `ID 7`, where
  `ID` is the head Keyword — does not: function dispatch is a `functions`
  bucket lookup with no placeholder consultation, so a call to a function
  not yet registered in the same scope surfaces as `DispatchFailed` rather
  than parking. Forward calls from a function *body* are unaffected: bodies
  re-dispatch per call, by which point every sibling binder has registered.
- **Placeholder install** (phase 3). If the picked function carries a
  `pre_run` extractor, `Resolved.placeholder_name` is its result and the
  driver installs `name → NodeId(idx)` on the dispatching scope. A
  `Rebind` collision here surfaces as a `Done(Err(_))` step so other slots
  keep draining.
- **Auto-wrap pass** (phase 4, carrier: `Resolved.slots.wrap_indices`). Promotes
  bare-name parts in *value-typed* slots of the picked function to
  single-part sub-expressions so they re-enter `run_dispatch` and route
  through the bare-name short-circuit. Both `ExpressionPart::Identifier`
  and bare leaf `ExpressionPart::Type` (a Type-token with no `<…>`
  parameters) are bare-name parts here and ride identical rails: `LET y = z`
  and `LET Ty = Number` walk the same wrap → sub-dispatch → `value_lookup`
  path, the first through the `Identifier` overload and the second
  through the `TypeExprRef` overload of `value_lookup`. Multi-name forward
  references compose as N independent sub-Dispatches.
- **Replay-park** (phase 4, carrier: `Resolved.slots.ref_name_indices`). Covers
  literal-name slots that *don't* sub-dispatch (`call_by_name`'s verb,
  `ATTR`'s identifier-lhs, `type_call`'s verb, ascription's `m` / `s`
  slots): if any of those names — Identifier or bare leaf Type-token —
  resolves to a placeholder whose producer hasn't terminalized, the outer
  slot's work is rewritten to `Dispatch(same_expr)` and parked on the
  producer's notify-list; on wake the re-dispatch finds the binding in
  `data` and proceeds. If the producer already terminalized with an error,
  the consumer's replay-park surfaces it with a `<replay-park>` frame
  rather than parking on a dead slot.

`Resolved.slots`'s three index vectors (`wrap_indices` / `ref_name_indices` /
`eager_indices`) are disjoint by construction: each slot's
`(SignatureElement, ExpressionPart)` shape lands in at most one bucket.
[`KFunction::classify_for_pick`](../src/machine/core/kfunction.rs) is
the sole producer of the `ClassifiedSlots` carrier (which `Resolved` holds
by value), so the disjointness invariant lives in one place rather than as
comment-enforced rules across the scheduler driver.

The bare-name short-circuit and replay-park call `DepGraph::add_park_edge`,
which records a `DepEdge::Notify(producer)` in the consumer's `dep_edges` entry
alongside the `DepEdge::Owned(child)` entries that mark sub-slots the consumer
owns. `add_park_edge` and its `add_owned_edge` sibling each install the
forward `notify_list[producer]` wake and the `pending_deps[consumer]` bump
atomically with the backward record, so a park-edge install is one atomic
+1 across the three vectors. `free()` recurses only into `Owned` arms, so a
consumer's reclamation cannot transit a park edge into a sibling producer's
subtree. Same-scope rebind of a value name surfaces
as `KErrorKind::Rebind`; an `FN` overload duplicating an existing exact
signature surfaces as `KErrorKind::DuplicateOverload`. Type bindings share
this placeholder mechanism: a type-binding site registers in
`Scope::placeholders` exactly like a value binding, external lookups park
the same way, and self-references during a binding's own elaboration
short-circuit through the elaborator's threaded-set recognition (see
[typing/elaboration.md](typing/elaboration.md)) so
recursive type definitions don't deadlock on their own placeholder.
FN-signature elaboration plugs into the same mechanism: when
[`elaborate_type_expr`](../src/machine/model/types/resolver.rs) hits a
bare type-name leaf whose binder is in
`Scope::placeholders` but not yet finalized, it returns
`ElabResult::Park(producers)` and FN-def's body schedules a `Combine`
over those producers that re-runs the signature elaboration against the
now-final scope at finish time. A parens-wrapped parameter type
(`xs: (LIST_OF Number)`) rides the same Combine: `parse_fn_param_list`
records the `(slot_idx, sub_expr)` pair, FN-def schedules each sub-expression
as its own `Dispatch`, and the Combine's finish closure splices each result
into `signature_expr.parts[slot_idx]` as `Future(KTypeValue(_))` before
re-running the parameter-list walk against the spliced signature. STRUCT and
UNION share the same elaborator-and-Combine shape for their field-type lists. The replay-park
rail itself cycle-checks before installing the park edge:
[`DepGraph::would_create_cycle`](../src/machine/execute/scheduler/dep_graph.rs)
walks the forward `notify_list` graph from the consumer and, if the
producer is reachable, the replay-park surfaces a `ShapeError("cycle in
type alias ...")` instead of installing the park edge. That catches the
trivially-cyclic case (`LET Ty = Ty` — the value-side `Ty` sub-Dispatch is
the LET binder's `Owned` child and is about to park on its own ancestor)
generically rather than as a special case in the elaborator.

`would_create_cycle` is a proactive check on the replay-park rail; the
bare-name short-circuit (phase 1) parks without it, so a value self-reference
(`LET x = x`, whose RHS `x` resolves through `Scope::resolve` to the binder's
own placeholder) still forms a cycle. A drain-end guard catches that and any
other cycle the proactive check doesn't: after [`execute`](../src/machine/execute/scheduler/execute.rs)
empties its work queues, it scans the slot table for nodes still parked
(`PreRun`) — a node parked on a dependency that can no longer fire — and
returns `KErrorKind::SchedulerDeadlock { pending, sample }` rather than letting
the top-level result read panic on an unresolved slot. `sample` is the source
expression of the first parked `Dispatch`/`Bind` node, so the diagnostic points
at code the reader can act on.

## `KObject` and the model/core boundary

[`KObject`](../src/machine/model/values/kobject.rs) is the universal
runtime value type. Pure-data variants (`Number`, `KString`, `Bool`,
`List`, `Dict`, `KExpression`, `*Type` schema carriers, `Tagged`,
`Struct`, `KTypeValue`, `TypeNameRef`, `Null`) carry no references
into [`machine::core`](../src/machine/core.rs). The runtime-reference
variants do — `KFunction`, `KFuture`, `KModule`, `KSignature`,
`Wrapped` embed `&'a KFunction<'a>`, `KFuture<'a>`,
`&'a Module<'a>`, `&'a Signature<'a>`, `&'a KType`, and an
`Option<Rc<CallArena>>` lifecycle anchor. These references are why
`model::values::kobject` imports from `core::{arena, kfunction,
scope, scope_id}`.

The references are structural, not incidental. Three hot consumers
read the concrete runtime shape directly:

- [`lift.rs`](../src/machine/execute/lift.rs) compares
  `f.captured_scope().arena` and `m.child_scope().arena` against the
  dying frame to decide whether a per-call function or module needs
  its `Rc<CallArena>` anchor cloned onto the lifted value.
- [`KObject::ktype()`](../src/machine/model/values/kobject.rs)
  synthesizes `KType::UserType { kind: Module, scope_id, name }` for
  module values from `m.scope_id()` and `m.path` — the dispatcher
  reads these fields to nominally identify the module type.
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
- **Per user-fn call.** `KFunction::invoke` clones the body
  (`expr.clone()` over the parts vector) so the slot has its own
  working copy for [the splice mechanism](#working-copy-splice).
  Clone cost is O(body size). It also acquires a per-call frame —
  either reusing the prev-step's `CallArena` shell via
  `try_reset_for_tail` (see [memory-model.md § Tail-step frame
  reuse](memory-model.md#tail-step-frame-reuse)) or allocating a fresh
  one. The reuse path is allocation-free; the fresh path heap-allocates
  one `Rc<CallArena>` plus six `typed_arena::Arena::new()` pools.
- **Per dep-result splice.** O(1) write into `expr.parts`.
- **Per terminal.** Single `notify_list` drain. The cost scales with
  the producer's dependent count, which is typically 1 (the parent
  Bind/Combine) but unbounded in principle (forward-reference parks).

### What amortizes

- **Slot recycling.** `Scheduler::reclaim_deps` frees sub-slots eagerly
  during `run_bind` / `run_combine` / `run_catch`, and `add()` pulls
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
  [memory-model.md § Tail-step frame reuse](memory-model.md#tail-step-frame-reuse).

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
