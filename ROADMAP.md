# Roadmap

Open structural items that don't fit in a single PR. Each entry below names the problem,
why it matters, and possible directions — not a fixed design. Per-item write-ups live in
[roadmap/](roadmap/).

The order matters. Sequencing is purely about technical and design dependencies — Koan has
no users yet, so backward-compatibility costs play no role. The cost being optimized is
engineering rework: doing one item before another it depends on means doing the dependent
item twice. Each per-item file ends with a **Dependencies** section linking to its
prerequisites and the items it unblocks.

Design rationale for what's already in the language lives in [design/](design/) — six
topical docs covering the execution model, memory model, functional programming, type
system, expressions and parsing, and error handling. A seventh design doc,
[design/module-system.md](design/module-system.md), captures the module-based
abstraction system end-to-end; stage 1 (the module language) shipped, and the
remaining six stages live as `module-system-*` roadmap items below. What's
shipped so far: user-defined functions, the dispatch-as-node scheduler refactor,
first-cut tail-call optimization, the leak fix (with lexical closures + per-call
arenas), structured error propagation, the user-defined-types substrate (return-type
enforcement at runtime), the IF-THEN→MATCH consolidation (`MATCH` accepts `Bool`
directly via projection at entry), per-parameter type annotations on user-fn
signatures, container type parameterization (`List<T>`, `Dict<K, V>`,
`Function<(args) -> R>`), transient-node reclamation (Bind/Aggregate sub-trees
recycled via a per-slot deps sidecar + free-list, keeping repeated-call scheduler
memory near-constant), per-call-frame chaining for builtin-built frames (MATCH's
child-scope `outer` no longer dangles when a TCO replace drops the call-site frame),
a targeted KFuture lift anchor (an addresses-only side-table on `RuntimeArena`
answers a precise membership query, replacing the previous always-anchor conservative
path), the leak-fix audit sign-off (a cycle gate on per-call `alloc_object`
redirects self-anchored values to the outer arena, closing out the audit slate at 0
leaks and 0 UB under Miri tree borrows), the quote/eval sigils (`#(expr)` and
`$(expr)` — surface forms that capture an AST as a `KExpression` value or evaluate a
`KExpression` value as code, closing the gap between "`KExpression` is first-class"
and "user code can manipulate expressions ergonomically"), the module-system
stage 0 cleanup (vestigial `KType::TypeRef` removed in favor of the unified
`TypeExprRef` slot kind, struct values now `IndexMap`-backed so PRINT emits fields
in declaration order, constructor dispatch funneled through a single
`dispatch_constructor` helper, and a `TypeResolver` trait threaded through
`KType::from_type_expr` ready for stage 1's module-aware resolver), and the
module-system stage 1 module language (`MODULE` and `SIG` declarators bind
structures and signatures under Type-token names; `:|` opaque ascription mints
fresh `KType::ModuleType { scope_id, name }` per declared abstract type so two
ascriptions of the same source module are observably distinct types; `:!`
transparent ascription shape-checks against the signature without re-tagging
identity; `Module`/`Signature` first-class values arena-allocated alongside
`KFunction` and reachable via `Foo.member` ATTR access). The next
signature revision after error handling lands monadic side-effect capture; the
type-system arc runs through the module-system stages — foundation now landed
in stage 1, ergonomic generic dispatch in stage 5, coherence in stage 6.

## Next items

Items with no unresolved roadmap-level prerequisites — any of these can be picked up
without first landing something else:

- [Generalize `Scope::out` into monadic side-effect capture](roadmap/monadic-side-effects.md)
  — `Scope::out` is one ad-hoc effect channel; every future effect (IO, time, randomness)
  needs a uniform carrier. (Previously a soft prerequisite of transient-node reclamation;
  now decoupled — reclamation shipped without touching `BuiltinFn`.)
- [Refactor for cleaner abstractions](roadmap/refactoring.md) — standing/exploratory; act
  only when the next feature would multiply existing duplication.

## Open items

### Memory and runtime substrate

- [Generalize `Scope::out` into monadic side-effect capture](roadmap/monadic-side-effects.md)
  — `Scope::out` is one ad-hoc effect channel; every future effect (IO, time, randomness)
  needs a uniform carrier.

### Module system

The agreed design is captured in [design/module-system.md](design/module-system.md);
stage 1 shipped (the module language: `MODULE`/`SIG` declarators, `:|`/`:!`
ascription, per-module type identity), and the remaining stages below land
the rest incrementally, each producing a usable end state.

- [Stage 1.5 — Scheduler integration](roadmap/module-system-1.5-scheduler.md) —
  `Infer` and `ImplicitSearch` scheduler nodes, the type-checking phase boundary,
  multi-target unification, and a post-stage-1 Miri audit slate re-run.
- [Stage 2 — Functors](roadmap/module-system-2-functors.md) — parametric modules with
  explicit application and sharing constraints.
- [Stage 4 — Property testing and axioms](roadmap/module-system-4-axioms-and-generators.md)
  — Rust-side property-testing engine kept disjoint from dispatch; axiom syntax in
  signatures with compile-time checking on ascription.
- [Stage 5 — Modular implicits](roadmap/module-system-5-modular-implicits.md) —
  implicit module parameters with lexical resolution and strict-on-ambiguity.
- [Stage 6 — Equivalence-checked coherence](roadmap/module-system-6-equivalence-checking.md)
  — cross-implicit equivalence testing; the differentiating coherence story.
- [Stage 7 — Syntax tuning and witness types](roadmap/module-system-7-syntax-tuning.md)
  — disambiguation sugar designed against patterns from real stage-5 code, plus opt-in
  witness types.

### Type system

- [Group-based operators](roadmap/group-based-operators.md) — `+`/`-` form a math group
  but the language treats every operator as a flat independent builtin. Generic
  dispatch over groups arrives with the module system's modular implicits.
- [Per-declaration type identity for structs and tagged unions](roadmap/per-declaration-type-identity.md)
  — `KType::Struct` and `KType::Tagged` are flat singletons, so two distinct
  `STRUCT` declarations report the same type. Extend per-declaration identity
  along the lines of the module system's `KType::ModuleType` carrier.

### Surface and ergonomics

- [Files and imports](roadmap/files-and-imports.md) — a Koan codebase is one file;
  no way for a `.koan` file to reach into another, and no story for how files become
  modules.
- [Error-handling surface follow-ups](roadmap/error-handling.md) — errors-as-values,
  source spans on `KExpression`, continue-on-error (independent), plus typed
  user errors and the catch surface (gated on module-system stage 2).

### Future-facing

- [Static type checking and JIT compilation](roadmap/static-typing-and-jit.md) — the
  tooling and performance ceiling; both want a phase between parse and execution.
- [Refactor for cleaner abstractions](roadmap/refactoring.md) — standing item: remove
  accidental abstraction when the next feature would multiply existing duplication.
