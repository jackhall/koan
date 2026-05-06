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

**Directions.** None decided.

- *Checker scope.* Soft inference (warnings only, execution proceeds), hard inference
  (rejected programs don't run), or gradual (typed regions check statically, untyped
  fall through to runtime dispatch). Gradual matches the type sequence's incremental
  shape — programs work without annotations, get checked once they have them.
- *JIT target.* Bytecode VM (modest speedup, manageable scope, Lua/Python pattern), native
  via cranelift or LLVM (real native speed, large toolchain dependency, hard to justify
  for a research language), or inline caching only (cache resolved dispatch targets at
  each call site, no IR, partial speedup). Inline caching is the cheapest first step
  that doesn't preclude either bigger option later.
- *Coupling.* Build the checker first, JIT later: checker output (resolved dispatch
  targets, monomorphized signatures) is exactly what a JIT consumes. Building the
  checker first ships independent value (errors, tooling) and produces the substrate the
  JIT later builds on. JIT-without-checker would duplicate type inference inline; avoid.
- *Closure interaction.* The leak fix's per-call arena + lexical closure model is the
  load-bearing memory shape. A checker's lifetime story and a JIT's codegen contract both
  have to honor it. Work through a closure-heavy test program before committing to an IR.

## Dependencies

**Requires:**
- [Open issues from the leak-fix audit](leak-fix-audit.md) — without a stable memory
  model, both the checker's understanding of value lifetimes and the JIT's codegen
  contract have nothing solid to target.
- [Module system stage 5 — Modular implicits](module-system-5-modular-implicits.md) —
  the type system has to be structurally complete before the checker is designed
  against it. Stage 5 is the latest stage that introduces new shapes the checker has
  to handle (modules, functors, first-class modules, implicit resolution); stages 6
  and 7 are additive.

Container type parameterization is shipped, so the checker has parameterized
containers to target.

The "if/when" is genuinely open — Koan may stay an interpreter forever and ship a
checker as tooling-only, or commit to a JIT eventually. Recording the option here so
design choices upstream don't accidentally close off either path.
