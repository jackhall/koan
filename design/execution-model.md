# Execution model: graph-based, dispatch separated from execution

Koan's runtime is a three-stage pipeline. Each top-level expression flows through:

```
source ──▶ parse ──▶ dispatch ──▶ execute
        KExpression   KFuture      KObject
```

Dispatch and execution are deliberately separate stages. **Dispatch** does
name-resolution and signature-matching: given a `KExpression` and a `Scope`, it
returns a [`KFuture`](../src/runtime/machine/core/scope.rs) — the resolved `&KFunction` plus
its `ArgumentBundle`, ready to run but not yet executed. **Execution** is what
the [`Scheduler`](../src/runtime/machine/execute/scheduler.rs) does: it owns a DAG of deferred
work, decides when each `KFuture` runs, and hands its body the live scope.

## Dispatch as a scheduler node

The scheduler models dispatch itself as a node type — `Dispatch(KExpression)`.
[`schedule_expr`](../src/runtime/machine/execute/interpret.rs) collapses to "add a `Dispatch`
node per top-level expression"; the rest is dynamic. At run time a `Dispatch`
walks its expression's parts, spawns sub-`Dispatch`/`Bind`/`Combine` nodes for
nested sub-expressions, and a builtin body holding `&mut dyn SchedulerHandle`
can also add `Dispatch` nodes.

`Combine` is the host-side dual of `Bind`: an N→1 combinator that waits on a
fixed set of dep slots and then runs an arbitrary host closure
([`CombineFinish`](../src/runtime/machine/kfunction.rs)) over their resolved values.
List- and dict-literal planners use it; the construction logic — including
already-resolved literal scalars that don't need a dep slot — lives in the
closure's capture rather than in fixed-shape variants. Body-finalization for
future MODULE/SIG inner work will reuse the same primitive.

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
rewritten to `Lift { from: NodeId }` (a [`NodeWork`](../src/runtime/machine/execute/nodes.rs)
variant). The Lift shim parks on the spawned `Bind`'s notify-list, waits
for that slot's terminal write, and copies the result into its own slot when
it runs. The original slot keeps its frame and notify-list across the
rewrite, so consumers downstream see the eventual terminal as if the body
had produced it directly.

## Push/notify dependency edges

The scheduler's edges point producer → consumer. Each slot carries a
`notify_list: Vec<NodeId>` of dependents waiting on it; each `Bind` /
`Combine` / `Lift` consumer carries a `pending_deps: usize` counter of
unresolved deps. When a slot writes a terminal `Value` or `Err`, the
notify-walk drains its `notify_list`, decrements each consumer's
`pending_deps`, and pushes any zero-counter consumer onto the run-set.
The terminal write and notify-walk fire in a single
[`Scheduler::finalize`](../src/runtime/machine/execute/scheduler/execute.rs)
method body that pairs `NodeStore::finalize` with `DepGraph::drain_notify`,
so the "every terminal write fires the notify" rule is type-enforced
rather than restated at each call site. Consumers arrive on the run-set
only when actually ready; there is no poll-and-requeue.

The run-set has two priority bands managed by
[`WorkQueues`](../src/runtime/machine/execute/scheduler/work_queues.rs). Internal
work — notify-walk wake-ups, Replace-arm re-enqueues, and ready-on-arrival
nodes registered in `add()` — routes through `WorkQueues::push_internal` /
`push_internal_front` / `push_woken`. Top-level `add_dispatch` calls route
through `WorkQueues::push_top_level` so independent top-level expressions
execute in submission order. The execute loop drains via `WorkQueues::pop_next`,
which yields internal slots ahead of top-level slots; the routing rule (which
band a push lands in) and the priority rule (which band a pop drains first)
are both enforced by the wrapper's method surface rather than restated at each
call site.

## Tail-call optimization

[`BodyResult::Tail(KExpression)`](../src/runtime/machine/kfunction.rs) makes a tail
return rewrite the **current scheduler slot's work** to a fresh
`Dispatch(expr)` and re-run in place — no new node allocated. Both deferring
builtins (`match_case`, `KFunction::invoke` for user-fns) are tail by
construction. A chain of tail calls (`A → B → PRINT`, or unbounded
`LOOP → LOOP`) reuses one slot end-to-end. Verified by two slot-count
assertions in the test suite.

A subtle point: host-stack overflow on naïve recursion is solved by the graph
model itself, not by `Tail`. Every "recursive call" enters the scheduler's
run-set rather than growing the Rust call stack — that property is
structural, not optimizing. What `Tail` adds is constant **scheduler-vec**
memory across the tail-call chain.

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
[`NodeStore`](../src/runtime/machine/execute/scheduler/node_store.rs)
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
[`DepGraph`](../src/runtime/machine/execute/scheduler/dep_graph.rs) sub-struct
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
same property: a lookup whose target binder has dispatched but not yet
executed parks on the producer instead of failing with `UnboundName`. The
mechanism lives in two pieces.

A `placeholders` table — a `RefCell<HashMap<String, NodeId>>` — lives
inside the [`Bindings`](../src/runtime/machine/core/scope.rs) façade on
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

The execute side — [`run_dispatch`](../src/runtime/machine/execute/scheduler/dispatch.rs) — is a
five-phase linear pipeline: a bare-name short-circuit, the chain-walked
resolution, the placeholder install, the auto-wrap + replay-park rewrite,
and the dep schedule. Phase 2 calls
[`Scope::resolve_dispatch`](../src/runtime/machine/core/scope.rs) once and
matches on its [`ResolveOutcome`](../src/runtime/machine/core/scope.rs):
`Resolved(r)` continues into phase 3 with the picked function plus the
per-slot index buckets `r.slots` carries (`wrap_indices`, `ref_name_indices`,
`eager_indices`); `Ambiguous(n)` and `Unmatched` surface as
`AmbiguousDispatch` / `DispatchFailed` errors; `Deferred` (no match against
the bare shape but the expression carries nested `Expression` /
`ListLiteral` / `DictLiteral` parts whose evaluation may produce typed
`Future(_)` parts that match) jumps to phase 5's eager-fallthrough loop and
re-dispatches via [`run_bind`](../src/runtime/machine/execute/scheduler/finish.rs)
after subs resolve.

The four rails the resolution feeds:

- **Bare-name short-circuit** (phase 1, runs before resolution). A
  single-`Identifier` dispatch slot (`(some_var)`) consults `Scope::resolve`
  directly: `Value` returns inline, `Placeholder` rewrites the slot's work
  to `Lift { from: producer_id }` (the same shim `BodyResult::Tail` uses for
  sub-Bind waits), `Unbound` falls through so `value_lookup`'s body
  produces the structured error.
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
  and `LET T = Number` walk the same wrap → sub-dispatch → `value_lookup`
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
[`KFunction::classify_for_pick`](../src/runtime/machine/kfunction.rs) is
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
[type-system.md § Type elaboration](type-system.md#type-elaboration)) so
recursive type definitions don't deadlock on their own placeholder.
FN-signature elaboration plugs into the same mechanism: when
[`elaborate_type_expr`](../src/runtime/model/types/resolver.rs) hits a
bare type-name leaf whose binder is in
`Scope::placeholders` but not yet finalized, it returns
`ElabResult::Park(producers)` and FN-def's body schedules a `Combine`
over those producers that re-runs the signature elaboration against the
now-final scope at finish time. STRUCT and UNION share the same
elaborator-and-Combine shape for their field-type lists. The replay-park
rail itself cycle-checks before installing the park edge:
[`DepGraph::would_create_cycle`](../src/runtime/machine/execute/scheduler/dep_graph.rs)
walks the forward `notify_list` graph from the consumer and, if the
producer is reachable, the replay-park surfaces a `ShapeError("cycle in
type alias ...")` instead of installing the park edge. That catches the
trivially-cyclic case (`LET T = T` — the value-side `T` sub-Dispatch is
the LET binder's `Owned` child and is about to park on its own ancestor)
generically rather than as a special case in the elaborator.

## Open work

- **Inference and search as scheduler work**
  ([module-system.md § Inference and search](module-system.md#inference-and-search-as-scheduler-work)).
  Type inference and modular-implicit resolution reduce to the existing
  `Dispatch` and `Bind` machinery — type-returning builtins on the value
  path, `Bind` as the refinement-and-wake-up mechanism, and stage 5
  implicit search as a single `SEARCH_IMPLICIT` builtin rather than a new
  node kind.
  [Eager type elaboration](../roadmap/eager-type-elaboration.md) lands the
  scheduler-driven type-elaboration substrate end-to-end through FN
  signatures, including placeholder-based recursive type definitions;
  module-system [stage 2](../roadmap/module-system-2-scheduler.md) layers
  higher-kinded slots and sharing constraints on top;
  [stage 5](../roadmap/module-system-5-modular-implicits.md) layers
  implicit search.
- **Monadic side-effect capture**
  ([roadmap/monadic-side-effects.md](../roadmap/monadic-side-effects.md)).
  `Scope::out` is one ad-hoc effect channel today; future effects (IO, time,
  randomness) need a uniform carrier that threads through the same node graph.
