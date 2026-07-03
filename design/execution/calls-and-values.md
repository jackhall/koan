# Calls, values, and performance

The value/runtime boundary and the lexical scaffolding around a call: the
`KObject` model/core boundary, the performance characteristics of the slot model,
and the lexical provenance chain that gives every dispatched node its scope. Part
of the [execution model](README.md).

## `KObject` and the model/core boundary

[`KObject`](../../src/machine/model/values/kobject.rs) is the universal
runtime value type — the `Object` arm of the scheduler's value currency
[`Carried`](../../src/machine/model/values/carried.rs); a type rides the
`Type` arm as a raw `&KType`, with no `KObject` box. Pure-data variants
(`Number`, `KString`, `Bool`, `List`, `Dict`, `KExpression`, `Tagged`,
`Record`, `Null`) carry no references into
[`machine::core`](../../src/machine/core.rs). The runtime-reference
variants do — `KFunction` and `Wrapped`
embed `&'a KFunction<'a>`, `&'a KType`, and an
`Option<Rc<FrameStorage>>` lifecycle anchor. (A module / signature value
travels the `Type` arm as `KType::Module { &Module, .. }` /
`KType::Signature { &Signature, .. }`, so those references live on `KType`,
not `KObject`.) These references are why `model::values::kobject`
imports from `core::{region, kfunction, scope, scope_id}`.

The references are structural, not incidental. Three hot consumers
read the concrete runtime shape directly:

- [`lift.rs`](../../src/machine/execute/lift.rs) compares
  `f.captured_scope().region` and `m.child_scope().region` against the
  dying frame to decide whether a per-call function or module needs
  its `Rc<CallFrame>` anchor cloned onto the lifted value — `lift_kobject`
  for the `Object` arm, `lift_ktype` for a `Type`-arm module/signature.
- [`KObject::ktype()`](../../src/machine/model/values/kobject.rs)
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
need the concrete region/scope/path identity that the abstraction
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
  working copy for [the splice mechanism](scheduler.md#working-copy-splice).
  Clone cost is O(body size). It also acquires a per-call frame —
  either reusing the prev-step's `CallFrame` shell via
  `try_reset_for_tail` (see
  [per-call-region/frames.md § TCO frame reuse](../per-call-region/frames.md#tco-frame-reuse))
  or allocating a fresh one. The reuse path is allocation-free; the
  fresh path heap-allocates one `Rc<CallFrame>` plus six
  `typed_arena::Region::new()` pools.
- **Per dep-result splice.** O(1) write into `expr.parts`.
- **Per terminal.** Single `notify_list` drain. The cost scales with
  the producer's dependent count, which is typically 1 (the consumer
  parked on it through a dep-finish or catch `cont`) but unbounded
  in principle (forward-reference parks, where the splice moves many
  consumers onto one producer).

### What amortizes

- **Slot recycling.** `Scheduler::reclaim_deps` frees sub-slots eagerly
  during [`run_step`](../../src/machine/execute/run_loop.rs), and `add()`
  pulls
  from the free-list before extending the underlying vectors. A
  steady-state recursive body reuses the same slot indices across
  iterations; `body_subexpression_slots_recycle_across_calls` pins the
  bound at ≤3 net slots/call.
- **Tail-call slot rewrite.** An `Action::Tail` (lowered to
  `Outcome::Continue`) rewrites the current slot's work in place rather than
  allocating a new one — one slot for an arbitrarily deep tail-call chain.
- **Tail-step frame reuse.** When the prev step's `CallFrame` is
  uniquely owned, `try_reset_for_tail` swaps its inner `KoanRegion`
  for a fresh one and re-binds — no `Rc<CallFrame>` box allocation,
  no `Scope` re-anchoring through the heap. See
  [per-call-region/frames.md § TCO frame reuse](../per-call-region/frames.md#tco-frame-reuse).

### Vs a tree-walking interpreter

A recursive descent on `&KExpression` would skip the slot table, edge
bookkeeping, and body clone — probably 5-10× faster on tight numeric
loops. What it can't do cheaply:

- **TCO.** Direct recursion grows the host stack; the koan model
  rewrites a slot in place. A tree-walker needs explicit trampolining
  with a worklist (which is roughly the slot table reinvented).
- **Forward references.** `LET y = (x); LET x = …` parks `y`'s
  sub-Dispatch on `x`'s producer via `NameLookup::Parked` and
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
[`LexicalFrame`](../../src/machine/core/lexical_frame.rs) recording its
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

### Single entry point: `KoanRuntime::enter_block`

Every dispatched node has a chain because every new lexical block is
entered through one primitive. `KoanRuntime::enter_block(scope_id,
statements, scope)` prepends a frame `(scope_id, i)` for each
statement `i` onto the current ambient chain and submits the
statements as dispatch nodes:

- Top-level statements
  ([`interpret`](../../src/machine/execute/runtime/interpret.rs)) enter through
  `enter_block(root.id, exprs, root)` against an empty parent chain.
- `MODULE` and `SIG` bodies enter through the dispatch harness's `InScope`
  fan-out
  ([`apply_outcome`](../../src/machine/execute/runtime.rs)), which splits
  via the shared
  [`split_body_statements`](../../src/machine/core/kfunction/body.rs) helper and
  submits each statement through `enter_block`. The scheduler itself never
  inspects AST shape — `split_body_statements` is the single source of truth for
  the split.
- FN, FUNCTOR, MATCH-arm, and TRY-arm bodies split via that same
  [`split_body_statements`](../../src/machine/core/kfunction/body.rs) helper
  (the all-`Expression` rule): the body's
  non-tail statements ride along as the `leading` field of an
  [`Action::Tail`](../../src/machine/core/kfunction/action.rs), and the slot
  parks on them as owned deps before tail-replacing into the last statement.
  Its `block_entry` names the body/arm scope; the harness derives the chain
  indices and the tail's `body_index` from `block_entry` + `leading`. TCO is
  preserved on the last statement. Single-statement bodies carry empty
  `leading` and tail-replace directly.
- FN bodies route through `run_user_fn` (see below — the chain
  shape is special because the call site's chain is not the body's
  lexical chain).

The "every dispatched node has a chain" invariant is an `expect` in
[`Scheduler::submit_node`](../../workgraph/src/scheduler/alloc.rs); the
public `dispatch_in_scope` entry auto-roots a chain when no ambient one is present
via [`LexicalFrame::detached`](../../src/machine/core/lexical_frame.rs) (so
REPL-style submissions outside `enter_block` see every prior bind in the target
scope).

### Multi-statement FN body split

A user-fn body of the shape `((s_0) (s_1) ... (s_{N-1}))` is split at
[`run_user_fn`](../../src/machine/core/kfunction/exec.rs) time (via
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
[Single entry point: `KoanRuntime::enter_block`](#single-entry-point-koanruntimeenter_block)
above).

### FN-body chain assembly

A function's body chain depth must equal the **lexical** nesting of
its definition site, not the **call** depth — otherwise tail-recursion
and mutual tail-recursion would grow the chain without bound.
[`assemble_body_chain`](../../src/machine/core/lexical_frame.rs) walks
the FN's captured `outer` scope chain (the lexical-definition path
set up by `CallFrame::new`) and, for each enclosing scope, looks it
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
[`try_body_let_not_visible_after_try`](../../src/builtins/try_with/tests.rs)).
This closes the **divergent-bind hazard** at the source level — a
binding visible only on one arm's runtime branch can't leak into the
enclosing block where its visibility would depend on which arm fired.

The **divergent-result hazard** is closed symmetrically on the result
side. `MATCH <v> -> :T WITH (...)` and `TRY (<e>) -> :T WITH (...)` carry
a mandatory declared return type `T` that every arm agrees on. The
selected arm tail-replaces carrying a
[`ReturnContract::Arm`](../../src/machine/core/kfunction/body.rs) on the
slot, and at the slot's Done step the scheduler's contract layer checks it against
`T` — [`TypeMismatch`](../../src/machine/core/kerror.rs) with a `<return>`
arg on a miss — then re-tags it to `T` so a downstream consumer dispatches
on the declared shape regardless of which arm ran. (The re-tag is the one
contract-driven relocation that survives at Done; the bare per-consumer lift
is a separate step — see
[per-call-region/lifecycle.md § Consumer-pull node-output lift](../per-call-region/lifecycle.md#consumer-pull-node-output-lift).) Enforcement is runtime
and per-arm (the arm that runs is the arm that's checked), the same
discipline FN return types follow — see
[typing/ktype/slots-and-signatures.md § Function signatures](../typing/ktype/slots-and-signatures.md#function-signatures).
`ReturnContract`
is the slot's return carrier: `Function(&KFunction)` for an FN / builtin
call, `Arm { ret, kind }` for a function-less MATCH / TRY arm.

### Read-side hook

The chain is read by name resolution through
[`LexicalFrame::index_for(scope_id)`](../../src/machine/core/lexical_frame.rs):
the lookup primitive that returns the consumer's statement index in a
given scope (or `None` when that scope is not on the chain — "already
returned", visibility unconstrained). The
[`Bindings::visible`](../../src/machine/core/bindings.rs) predicate consumes it as
`b.idx < cutoff` — one rule across the value and type languages; the
value-side `Scope::resolve_with_chain`, the type-side `resolve_type_with_chain`, the
bare-identifier `lookup_with_chain`, and the per-scope
[`Bindings::lookup_value`](../../src/machine/core/bindings.rs) /
`lookup_type` / `lookup_function` lookups (the last covering both the
overload-bucket filter and the in-flight `pending_overloads` fall-through
in one pass) all filter through it. The gate is `chain = None`-bypassed
for test fixtures and builtin-registration paths.

## Open work

- **Inference and search as scheduler work**
  ([typing/scheduler.md](../typing/scheduler.md)).
  Type inference and modular-implicit resolution reduce to the existing
  dispatch-decide and dep-finish machinery — type-returning builtins on the value
  path, a dep-finish `cont` as the refinement-and-wake-up mechanism, and stage 5
  implicit search as a single `SEARCH_IMPLICIT` builtin rather than a new
  node kind. Higher-kinded slots and sharing constraints layer on top of
  the scheduler-driven elaborator (see
  [typing/](../typing/README.md));
  [stage 5](../../roadmap/predicate_typing/modular-implicits.md) layers
  implicit search.
- **Monadic side-effect capture**
  ([roadmap/monadic-side-effects.md](../../roadmap/libraries/monadic-side-effects.md)).
  `Scope::out` is one ad-hoc effect channel today; future effects (IO, time,
  randomness) need a uniform carrier that threads through the same node graph.
