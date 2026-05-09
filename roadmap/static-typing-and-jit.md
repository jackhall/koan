# Static type checking and JIT compilation

**Problem.** Koan is purely interpreted: the scheduler walks the dispatch tree at runtime
and every type check happens inside the dispatcher's signature comparator. Two related
limitations follow.

- *Late error surfacing.* A type mismatch only fires when a value reaches an incompatible
  slot — possibly deep in a deferred dispatch tree, several frames from the call site
  that introduced the wrong type. The user sees the error at the leaf, not the source.
  Refactoring a function signature requires running every code path that touches it to
  find every misuse.
- *Per-node interpretive overhead.* The tree-walker pays a constant cost on every node
  (slot lookup, signature comparison, frame allocation) even for monomorphic call sites
  where every type is known statically. There is no specialization layer that could
  collapse a chain of fully-typed dispatches into a straight-line sequence of operations.

The two share a substrate: both want a phase between parse and execution that resolves
types and dispatch targets where it can. A checker is that phase emitting errors; a JIT
is the same phase emitting code.

**Impact.**

- *Tooling substrate.* Editor integrations get types, jump-to-dispatch-target, and
  pre-run error surfacing — the IDE-tier features mature languages take for granted
  unlock once the checker exists as substrate.
- *Performance ceiling lifts.* Hot paths get specialization — a chain of fully-typed
  dispatches collapses into straight-line operations instead of paying the tree-walker's
  per-node overhead. Not load-bearing today (no production users, no benchmark target)
  but the option opens for when it matters.

**Directions.**

- *Checker scope — open.* The user-facing choice is whether the checker permits
  unresolved type bindings — the
  [dispatch-time-placeholders](dispatch-time-placeholders.md) mechanism reaching
  through into the check phase — or insists every type identifier resolves at
  compile time. Permissive matches the dynamic-dispatch ergonomics today's runtime
  exhibits and gives the checker a soft-rejection mode for programs that work but
  can't be fully statically resolved; strict matches what a separate-from-runtime
  type system would conventionally enforce. Likely a per-build switch.
- *JIT target — decided.* Compilation and execution share a process, so the artifact
  to persist is a serialized scheduler-plus-ownership-state snapshot with pegged
  nodes (resolved dispatch targets, monomorphized signatures) pre-baked — not a
  separate bytecode IR, native object file, or inline-cache sidecar.
- *Coupling — decided.* Build the checker first, JIT later: checker output (resolved
  dispatch targets, monomorphized signatures) is exactly what the snapshot's pegged
  nodes are. Building the checker first ships independent value (errors, tooling)
  and produces the substrate the JIT later builds on. JIT-without-checker would
  duplicate type inference inline; avoid.
- *Closure interaction — decided.* The leak fix's per-call arena + lexical closure
  model is the load-bearing memory shape. A checker's lifetime story and a JIT's
  codegen contract both have to honor it. Work through a closure-heavy test program
  before committing to a snapshot format.

## Dependencies

**Requires:**
- [Module system stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md)
  — module-system stage 1 plus stage 2's module-and-functor work reshapes the
  memory model, and the stage-2 audit slate re-run is what re-establishes the
  sign-off the checker's lifetime story and the JIT's codegen contract both
  want to target. Stage 2 also lands the type-expression-as-`Dispatch` reduction
  the checker's IR shape depends on.
- [Module system stage 5 — Modular implicits](module-system-5-modular-implicits.md) —
  the type system has to be structurally complete before the checker is designed
  against it. Stage 5 is the latest stage that introduces new shapes the checker has
  to handle (modules, functors, first-class modules, implicit resolution); stages 6
  and 7 are additive.
- [`KType` and dispatcher concern split](ktype-and-dispatcher-split.md) — a
  clean dispatcher boundary lets the checker call into overload resolution
  without coupling to `Scope`'s storage internals, and the split-out
  resolution module gives the checker a focused surface for type-expression
  elaboration.

Container type parameterization is shipped, so the checker has parameterized
containers to target.

The "if/when" is genuinely open — Koan may stay an interpreter forever and ship a
checker as tooling-only, or commit to a JIT eventually. Recording the option here so
design choices upstream don't accidentally close off either path.
