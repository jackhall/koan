# Dispatch-time name placeholders

Install a name → producer-`NodeId` placeholder in `Scope` when a binder
dispatches, before its RHS is scheduled, so a lookup that outruns the
binder's completion parks on the producer instead of failing with an
unresolved-name error.

**Problem.** Every binder builtin (`LET`, `FN`, `STRUCT`, `SIG`, `UNION`,
`MODULE`) inserts its name → value pair only when its body runs —
`scope.add` lives in the body
([let_binding.rs:39](../src/dispatch/builtins/let_binding.rs#L39), and
the parallel call sites in `fn_def.rs`, `struct_def.rs`, `union.rs`,
`sig_def.rs`, `module_def.rs`). The body runs *after* its sub-deps
terminalize. Today this is invisible because top-level dispatches drain
FIFO ([scheduler.rs:42-43](../src/execute/scheduler.rs#L42-L43)) and
each top-level's full subtree completes before the next top-level
dispatches, so a later top-level's lookup of an earlier binder's name
always finds it. As soon as that ordering relaxes — multi-file imports
that aren't in topological order, MODULE-body members dispatched
concurrently rather than sequentially, FN-signature elaboration that
needs a type identifier whose binder hasn't run yet — there is no
mechanism: the lookup hits an unresolved name and dispatch fails.

**Impact.**

- *Forward references resolve regardless of dispatch order.* A lookup
  whose target binder has been dispatched but not yet executed parks on
  the producer's slot via the existing `notify_list` / `pending_deps`
  machinery. Once the binder terminalizes, the consumer wakes and
  resumes.
- *MODULE bodies, multi-file imports, and concurrent top-level
  expressions compose without order constraints.* Sibling-to-sibling
  references inside a composite (members of a single module, names
  imported across files, top-level expressions whose dispatch order is
  not their dependency order) work without the caller having to
  topo-sort source.
- *FN-signature deferred resolution simplifies.* The hybrid
  `OnceCell<KType>` per-slot fallback in
  [stage 2's signature elaboration](module-system-2-scheduler.md)
  handles bare type identifiers not yet bound at FN-def time. With
  dispatch-time placeholders, that case becomes a special case of the
  general mechanism; whether stage 2 retains the `OnceCell` as an
  optimization or migrates is a stage-2 design choice when this lands.

**Directions.**

- *Binder opt-in form — open.* The decided principle is that each binder
  builtin (`LET`, `FN`, `STRUCT`, `SIG`, `UNION`, `MODULE`) opts in to
  declaring its name at dispatch. Two implementation shapes:
  - A `Body::pre_run` hook on `KFunction` that the `run_dispatch` path
    invokes before scheduling sub-deps.
  - A `run_dispatch` special-case that recognizes binding-shaped
    expressions and installs the placeholder directly.

  The cost is paid only by the six binding sites in
  `src/dispatch/builtins/`.
- *Placeholder representation — decided.* A new
  `Scope::placeholders: RefCell<HashMap<String, NodeId>>` keyed by name,
  value the producer slot's `NodeId`. Keeps `Scope::data` value-typed so
  every `&KObject` consumer is unaffected. `lookup` checks
  `placeholders` after `data`. Whether the install path needs the
  `try_borrow_mut` + `pending`-queue mirror of `Scope::add`
  ([scope.rs:137-195](../src/dispatch/runtime/scope.rs#L137-L195))
  depends on whether a dispatch-time install can re-enter while
  `placeholders` is borrowed up-stack — settle when the implementation
  surface is concrete.
- *Deferred-edge install via `Replace` — decided.* A body whose lookup
  hits a placeholder cannot mutate the scheduler synchronously: bumping
  its own `pending_deps` and re-parking on `ready_set` is disallowed by
  the run loop's invariants. The body returns a `Replace`-shaped result
  whose new work parks the slot on the producer's `notify_list` — the
  same shape the existing `Lift { from: NodeId }` rewrite uses for
  sub-Bind waits. The execute loop's existing Replace path installs the
  edge via `register_slot_deps`
  ([scheduler.rs:172-189](../src/execute/scheduler.rs#L172-L189)).
- *Notify-only edges — decided.* The new edge is consumer→producer for
  *waking*, not for ownership. `notify_list[producer].push(consumer)`
  and `pending_deps[consumer] += 1`; `node_dependencies` (parent →
  owned-children, used by `free()`) is not touched, since the
  looking-up consumer does not own the binder's subtree.
- *Lookup miss for genuinely unbound names — decided: error, unchanged.*
  Placeholders only intercept lookups when a binder has *already*
  claimed the name. A miss against both `data` and `placeholders`
  continues to surface as today's unresolved-name dispatch error.
- *Rebind in same scope — decided: error.* Matches Haskell's same-scope
  duplicate-name rule and koan's value-immutability stance. Shadowing
  across scopes (nested call frames, child scopes built by binders)
  remains allowed — that is lexical scoping, not rebinding.
- *Type-RHS and value-RHS share placeholder semantics — decided.* A
  lookup that hits a placeholder always parks on the producer,
  regardless of whether the consumer is a value-builder or a
  type-builder. Consequence: recursive type definitions
  (`STRUCT T = (Field of T)` and similar) deadlock — the type-builder is
  the producer and cannot itself produce until its body resolves.
  Recursive type definitions remain a separate open question for a
  follow-up; the uniform-park rule keeps v1 small.
- *Cycle detection — out of scope for v1.* Unsatisfiable forward
  references (a cycle, or a reference to a name that no binder ever
  installs) surface as latent unresolved bindings rather than a
  structured cycle error. Detection is a follow-up if it becomes
  load-bearing.

## Dependencies

**Requires:**

**Unblocks:**

No hard prerequisites and no roadmap items downstream. The mechanism
uses the existing scheduler machinery (`notify_list`, `pending_deps`,
the `Replace` path) and the `try_borrow_mut` + `pending` re-entrancy
precedent in `Scope::add`. Module-system stage 2's hybrid
`OnceCell<KType>` resolver for bare type identifiers becomes a special
case of the general mechanism when this lands, but neither item blocks
the other.
