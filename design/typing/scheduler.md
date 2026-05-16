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
  `DICT_OF`, `FUNCTION_OF`, `MODULE_TYPE_OF` and the like dispatch and
  execute on the value path; their result is the elaborated type carried
  in `KObject::KTypeValue(KType)`. A `LET MyList = (LIST_OF Number)`
  binding finalizes once and makes `MyList` available as a type name in
  subsequent FN signatures with no per-lookup re-elaboration.
- **Type expressions in source position re-elaborate to a synthesized
  call.** A parameter or return type written as `(LIST_OF Number)` (or
  `:(List Number)`) is dispatched directly as a sub-expression whose value
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
