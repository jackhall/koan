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

- **Type-returning builtins are ordinary builtins.** `LIST_OF`,
  `DICT_OF`, `MODULE_TYPE_OF` and the like dispatch and
  execute on the value path; their result is the elaborated type carried
  in `KObject::KTypeValue(KType)`. A `LET MyList = (LIST_OF Number)`
  binding finalizes once and makes `MyList` available as a type name in
  subsequent FN signatures with no per-lookup re-elaboration.
- **Type expressions in source position re-elaborate to a synthesized
  call.** A parameter or return type written as `(LIST_OF Number)` (or
  `:(LIST OF Number)`) is dispatched directly as a sub-expression whose value
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

## Post-walk dispatch fallback precedence

When [`Scope::resolve_dispatch_with_chain`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
walks every visible scope without any bucket admitting strictly, it falls
through to a cache-driven post-walk fallback that picks one of five
outcomes by fixed precedence:

**Placeholders > eager parts > Unbound > pending overload > Unmatched.**

1. **Placeholders** — any `NameOutcome::Parked(_)` in the `bare_outcomes`
   cache ⇒ `ResolveOutcome::ParkOnProducers` on the deduplicated producer
   list. Wake re-dispatches; strict admission rebuilds the cache against
   the now-bound type.
2. **Eager parts** — otherwise, if some candidate overload would admit once
   its unevaluated eager-shaped parts (`Expression` / `SigiledTypeExpr` /
   `ListLiteral` / `DictLiteral` / `RecordLiteral`) evaluate — i.e. it admits
   *modulo eager* (`signature_admits(.., modulo_eager: true)`, which treats an
   eager part in an argument slot as satisfiable) ⇒ `Deferred`. The driver
   sub-Dispatches the eager parts and re-resolves against the spliced
   expression. A candidate that rejects on an already-evaluated slot fails this
   test, so an eager part that can't change any candidate's admission doesn't
   trigger a futile deferral.
3. **Unbound** — otherwise, any `NameOutcome::Unbound(name)` ⇒
   `UnboundName(name)`.
4. **Pending overload** — otherwise, an innermost-visible
   `pending_overloads[key]` recorded during the walk ⇒
   `ParkOnProducers(vec![producer])` (a sibling FN / FUNCTOR parked on
   its own Combine has installed the bucket; wake re-dispatches against
   the now-registered overload).
5. **Unmatched** — otherwise, `ResolveOutcome::Unmatched`.

**Why eager can outrank Unbound.** An Expression-in-slot dispatch like
`(maybe) some 42` has a head that *does* resolve, but only after one
sub-Dispatch evaluates `(maybe)` to the schema — there the rejecting slot is
itself the eager part, so the candidate admits modulo eager and the deferral is
warranted. Surfacing `UnboundName` on the unresolved sibling instead would
pre-empt that sub-Dispatch and report the wrong diagnostic. The modulo-eager
gate keeps that first-refusal honest: when no candidate can admit even after its
eager parts evaluate — the failure is an already-evaluated slot, e.g. a non-record
operand to [`FROM`](../../src/builtins/record_projection.rs) or a bare anonymous
`UNION (…)` — the fallback skips the deferral and surfaces the precise `Unbound` /
`Unmatched` diagnostic at the call, rather than eagerly evaluating an unrelated
operand and leaking its error.

The companion driver-side view of this precedence — what each outcome
routes to in the dispatch pipeline — lives at
[execution-model.md § post-walk fallback](../execution-model.md#dispatch).
