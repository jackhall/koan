# Inference and search as scheduler work

Type inference and implicit search interleave with dispatch and execution
in the same scheduler. Inference of an expression may need an implicit
search to resolve a parameter; the search may refine type slots that
other inferences are waiting on; sub-expressions are dispatched and
executed by the same engine. There is no separate type-checking pass.
See [execution-model.md § Pegged and free execution](../execution-model.md#pegged-and-free-execution) —
build-time and run-time are the same engine, differing only in which nodes
are pegged.

**Inference and search reduce to the existing `Dispatch` and `Bind`
machinery.** Type expressions are evaluated by the same engine values
are. There is no `Infer` node kind; there is no `ImplicitSearch` node
kind; there is no `KType::TypeVar`.

The mechanism:

- **Type-returning builtins are ordinary builtins.** The keyworded type
  constructors (`LIST OF`, `MAP _ -> _`) and the like dispatch and
  execute on the value path; their result is the elaborated type carried
  in `KObject::KTypeValue(KType)`. A `LET MyList = :(LIST OF Number)`
  binding finalizes once and makes `MyList` available as a type name in
  subsequent FN signatures with no per-lookup re-elaboration.
- **Type expressions in source position re-elaborate to a synthesized
  call.** A parameter or return type written as `:(LIST OF Number)`
  is dispatched directly as a sub-expression whose value
  is a `KType`. Bare type identifiers in FN signatures park on the
  binding's scheduler placeholder via the same `notify_list` /
  `pending_deps` machinery value-name forward references use; recursive
  type definitions short-circuit self-references through the elaborator's
  threaded-set recognition rather than parking on their own placeholder
  ([elaboration.md](elaboration.md)).
- **Refinement rides on `Bind`.** A `Bind` waiting for its sub-Dispatches
  to complete is the existing wake-up mechanism; a type expression that
  tightens later (e.g. as functor application reaches the body) wakes
  its dependents through the same path.
- **Implicit search lands as a single new builtin `SEARCH_IMPLICIT`,
  not a new node kind.** Implicit resolution becomes a Dispatch against
  that builtin with the candidate set assembled from lexical scope; the
  result is a module value, threaded into the call site like any other
  argument.

Rejected: a parallel `Infer` / `ImplicitSearch` node-kind track, with
its own substitution table and `KType::TypeVar`. It would duplicate
scheduling, dependency tracking, cycle detection, and error
propagation that `Dispatch` and `Bind` already provide, and it would
fork the module language away from the value language at exactly the
point — inference — where the metacircular reuse is most valuable.

Properties this preserves:

- **Cycle detection is uniform.** A cycle in implicit resolution and a
  runtime infinite loop are the same kind of bug to the scheduler.
- **Topological ordering falls out.** Dependency-driven wake-up is the
  scheduler's job; type tasks ride the same edges value tasks do.
- **Failure isolation.** Inference and search failures propagate to
  dependents through the existing error-propagation rules; independent
  subtrees still finish, so the user sees multiple errors per build.

This is the **weak metacircular** form: the same scheduler engine that
runs Koan value-language work runs the type checker. The strong form
(compile-time tasks written in Koan and executed by the scheduler) is not
a goal; the architecture leaves the door open without paying its
bootstrapping cost.

The cost-side concession: refinement is first-time-ready (a type tightens
before its dependents fire), not tighten-after-the-fact (a type tightens
after its dependents have already run). The implicit-search work in
[open-work.md](open-work.md) is the most plausible motivation for a
tighten-after-the-fact scheduler primitive.

## In-walk dispatch precedence

[`Scope::resolve_dispatch_with_chain`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
walks visible scopes innermost-first and decides each scope's contribution
from its [`FunctionLookup`](../../src/machine/core/bindings.rs) — finalized
overloads and the earliest-visible in-flight pending producer, surfaced
together. The innermost scope that reaches a *terminal* decision wins, so
lexical shadowing holds: an inner overload — finalized, pending, or
admitting-once-its-eager-part-evaluates — shadows outer-scope overloads
regardless of finalize or evaluation order. The per-scope precedence:

1. **Visible pending ⇒ park.** A pending sibling on this scope's bucket key
   would shadow any finalized overload here once it finalizes, so the scope
   parks (`ParkOnProducers`) — even over a finalized strict-Pick at the same
   scope (Decision 5 below). Any forward-reference producers the relaxed pass
   would lean on union into the park list so a single wake re-runs the full
   resolution.
2. **Strict Pick / Tie.** Over the finalized overloads, the strict gate
   [`OverloadBucket::pick_strict`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
   Picks the most-specific admitting candidate (`Resolved`), or surfaces a
   genuine tie as `Ambiguous` — except a tie with an unevaluated eager part
   `Defer`s, since the eager value may break it.
3. **Strict-Empty ⇒ one relaxed-admission pass per candidate.** When no
   finalized overload strictly admits, each candidate runs a single relaxed
   pass that assumes every *unresolved* slot satisfiable and records which kind
   it leaned on: a `Parked` bare name (a producer exists), an unevaluated eager
   part, or a `Dead` unbound bare name (no producer will ever bind it). A
   candidate that rejects on a hard already-resolved / literal / keyword slot
   admits nothing even relaxed and contributes nothing. The leaned-on kinds
   resolve by precedence: a **parked** lean ⇒ `ParkOnProducers`; otherwise an
   **eager** lean ⇒ `Deferred`; otherwise a **dead** lean records an
   `UnboundName` blocker without terminating the walk.

Only two outcomes are decided *post-walk*, after every scope reported
`Continue`:

- **`UnboundName(name)`** when some scope's relaxed pass leaned on a dead
  unbound name. It is held back rather than terminating at the scope so an
  outer scope can still strict-Pick the bare name shape-only as an
  `:Identifier` / `:Any` slot — a dead inner lean must not pre-empt an outer
  Pick.
- **`Unmatched`** when no scope contributed even a dead lean.

**One relaxed pass covers parked and eager slots uniformly.** The relaxed pass
([`relaxed_admits`](../../src/machine/execute/dispatch/resolve_dispatch.rs))
treats a parked bare name as just an eager part whose value arrives from a
producer instead of a sub-Dispatch, so both kinds flow through the same
per-candidate classification. The dead-slot arm only labels the `UnboundName`
terminal — an unbound name never arrives, so it never triggers a wait. This
keeps the deferral honest: when no candidate can admit even relaxed — the
failure is a hard slot, e.g. a non-record operand to
[`FROM`](../../src/builtins/record_projection.rs) or a bare anonymous
`UNION (…)` — the walk surfaces the precise `UnboundName` / `Unmatched`
diagnostic at the call rather than eagerly evaluating an unrelated operand and
leaking its error.

**Decision 5: a bucket mixing finalized and pending parks until finalize.**
[`try_install_pending_overload`](../../src/machine/core/bindings.rs) records a
pending sibling *alongside* any live finalized overload on the same bucket key,
so `FunctionLookup` can surface both. When it does, the scope always parks until
the pending finalizes, then re-resolves the now-complete bucket — resolving
early when a finalized candidate is unambiguously most-specific is a later
optimization.

The companion driver-side view — what each outcome routes to in the dispatch
pipeline — lives at
[execution-model.md § post-walk fallback](../execution-model.md#dispatch).
