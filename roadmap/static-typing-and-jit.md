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

- *Tooling ceiling.* Without a checker, an editor integration can't show types, jump to
  dispatch targets, or surface errors before run. Every IDE-tier feature mature languages
  take for granted requires the checker as substrate.
- *Performance ceiling.* Tree-walking interpreters land near Python's interpreted speed.
  For Koan to ever be competitive on a real workload, hot paths need specialization. Not
  a problem today (no production users, no benchmark target) but it caps the language's
  eventual reach.

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
- [Container type parameterization](container-type-parameterization.md)
- [Per-type identity for structs and methods](per-type-identity.md)
- [`TRAIT` builtin for structural typing](traits.md)
- [Trait inheritance](trait-inheritance.md)

The type system has to be structurally complete (concrete types, parameterized
containers, per-type identity, traits, inheritance) before the checker is designed
against it, or the checker gets reworked as the type system grows. The "if/when" is
genuinely open — Koan may stay an interpreter forever and ship a checker as tooling-only,
or commit to a JIT eventually. Recording the option here so design choices upstream don't
accidentally close off either path.
