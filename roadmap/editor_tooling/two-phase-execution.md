# Two-phase execution: build-time with pegged inputs, run-time resume

**Problem.** The design model
([execution/scheduler.md § Pegged and free execution](../../design/execution/scheduler.md#pegged-and-free-execution),
[typing/scheduler.md](../../design/typing/scheduler.md)) is that build-time and
run-time are the **same scheduler engine**, differing only in which nodes
are pegged (held without execution until external data or effects arrive).
The intermediate representation is the **stalled DAG state** — `NodeStore`
and `DepGraph` contents at the free-execution fixed point, plus the
identifiers of pegged nodes. The scheduler engine exists; the build-time /
run-time split does not. Today every source file is parsed, dispatched,
and executed at run start; nothing distinguishes a node whose result
depends on user input from a node whose result is fully known at build
time, and nothing persists the work the build-time phase could have done.

Two consequences follow:

- *Late error surfacing.* A type mismatch only fires when a value reaches
  an incompatible slot — possibly deep in a deferred dispatch tree, several
  frames from the call site that introduced the wrong type. The user sees
  the error at the leaf, not the source. Refactoring a function signature
  requires running every code path that touches it to find every misuse.
  Build-time scheduling with input nodes pegged would surface every error
  that doesn't depend on the pegged input — most errors — before the
  program is run.
- *Per-node interpretive overhead.* The tree-walker pays a constant cost
  on every node (slot lookup, signature comparison, frame allocation) even
  for monomorphic call sites where every type is known statically. Build-
  time scheduling already pays this cost; persisting the stalled DAG state
  and resuming from it at run-time would not.

Both fall out of the same mechanism — there is no separate "checker phase"
to build first and "JIT phase" to build second. A third consequence is
that nothing can consume the build phase's results for code generation:
[design/compilation.md](../../design/compilation.md) describes the
compilation model that treats the stalled DAG state as its intermediate
representation, and it has no build phase to consume until this item
ships.

**Acceptance criteria.**

- The build-time scheduler run emits build-time errors,
  jump-to-dispatch-target data, and pre-run type information for the editor
  as direct outputs, without a separate analysis pass.
- A chain of fully-typed dispatches resolved during the build-time run
  resumes from the already-resolved DAG nodes at run-time, without re-paying
  the tree-walker's per-node overhead a second time at run start.

**Directions.**

- *Peg-set scope — open.* Which categories of node count as pegged at
  build time is enumerated in
  [execution/scheduler.md § Pegged and free execution](../../design/execution/scheduler.md#pegged-and-free-execution)
  in principle — user-supplied input, plugin source files, syscalls,
  network calls. Which concrete builtins / `KObject` shapes carry the peg
  marker, and whether the marker is intrinsic to the builtin or attached
  by the build-time driver, remains to be worked out.
- *Snapshot format — decided.* The artifact is a serialized
  scheduler-plus-ownership-state snapshot: `NodeStore`, `DepGraph`, the
  region heap-pinning chain, plus the identifiers of pegged nodes. Not a
  separate bytecode IR; not a native object file; not an inline-cache
  sidecar. Run-time consumes the snapshot directly, supplies the pegged
  inputs and effects, and the scheduler resumes.
- *Residual code generation — open.*
  [design/compilation.md](../../design/compilation.md) describes a second
  consumer of the same stalled state: a code generator emitting
  ahead-of-time code for the residual, with direct calls where the build
  phase resolved dispatch, table dispatch through the runtime library
  where it didn't, and an embedded evaluator for runtime-`EVAL` regions.
  This does not reopen the snapshot-format decision — the stalled DAG
  stays the sole intermediate representation; codegen is a back end over
  it. Open: whether the generator lands inside this item or as a
  follow-up item once the build phase exists, and what the first target
  is (transpiled Rust vs native).
- *Permissive vs strict build-time errors — open.* The user-facing choice
  is whether the build-time phase permits unresolved type bindings — the
  [dispatch-time name placeholder](../../design/execution/name-placeholders.md#dispatch-time-name-placeholders)
  mechanism reaching across into build-time — or insists every type
  identifier resolves before the snapshot is taken. Permissive matches the
  dynamic-dispatch ergonomics today's runtime exhibits and gives a soft-
  rejection mode for programs that work but can't be fully resolved at
  build time; strict matches what a conventional separate-from-runtime
  type system would enforce. Likely a per-build switch.
- *Closure interaction — decided.* The leak fix's per-call region + lexical
  closure model is the load-bearing memory shape. Snapshot format and
  resume path both have to honor it. Work through a closure-heavy test
  program before committing to a snapshot format.
- *Whether to commit at all — open.* Koan may stay single-phase indefinitely
  and surface build-time errors as a tooling-only mode, or commit to the
  snapshot-and-resume path. The residual-code-generation path rides on the
  build phase, so staying single-phase forfeits it too. Keep upstream
  design choices from closing off either path.

## Dependencies

Container type parameterization has shipped, so the build-time phase has
parameterized containers to target.

**Requires:**

- [Module system stage 5 — Modular implicits](../predicate_typing/modular-implicits.md) —
  the type system must be structurally complete first; stage 5 is the last stage that
  adds new shapes the build-time phase has to handle.

**Unblocks:** none.
