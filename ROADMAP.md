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
system, expressions and parsing, and error handling. One forward-looking design doc,
[design/module-system.md](design/module-system.md), captures the agreed module-based
abstraction system; it spans the seven `module-system-*` roadmap items below. What's
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
leaks and 0 UB under Miri tree borrows), and the quote/eval sigils (`#(expr)` and
`$(expr)` — surface forms that capture an AST as a `KExpression` value or evaluate a
`KExpression` value as code, closing the gap between "`KExpression` is first-class"
and "user code can manipulate expressions ergonomically"). The next signature revision after error handling lands monadic side-effect
capture; the type-system arc runs through the module-system stages — foundation in
stage 1, ergonomic generic dispatch in stage 5, coherence in stage 6.

## Next items

Items with no unresolved roadmap-level prerequisites — any of these can be picked up
without first landing something else:

- [Generalize `Scope::out` into monadic side-effect capture](roadmap/monadic-side-effects.md)
  — `Scope::out` is one ad-hoc effect channel; every future effect (IO, time, randomness)
  needs a uniform carrier. (Previously a soft prerequisite of transient-node reclamation;
  now decoupled — reclamation shipped without touching `BuiltinFn`.)
- [Module system stage 0 — Pre-module cleanup](roadmap/module-system-0-cleanup.md)
  — vestigial-tag removal, ordered struct values, centralized constructor dispatch,
  scope-aware type resolution. Foundation cleanup before stage 1 so the type-identity
  surgery there stays local.
- [Refactor for cleaner abstractions](roadmap/refactoring.md) — standing/exploratory; act
  only when the next feature would multiply existing duplication.

## Open items

### Memory and runtime substrate

- [Generalize `Scope::out` into monadic side-effect capture](roadmap/monadic-side-effects.md)
  — `Scope::out` is one ad-hoc effect channel; every future effect (IO, time, randomness)
  needs a uniform carrier.
- [Post-stage-1 Miri audit redo](roadmap/post-stage-1-audit-redo.md) — re-run the
  audit slate after module-system stage 1 reshapes the memory model.

### Module system

The agreed design is captured in [design/module-system.md](design/module-system.md);
the seven stages below land it incrementally, each producing a usable end state.

- [Stage 0 — Pre-module cleanup](roadmap/module-system-0-cleanup.md) — vestigial-tag
  removal, ordered struct values, centralized constructor dispatch, scope-aware type
  resolution. Engineering housekeeping before stage 1 starts.
- [Stage 1 — Module language](roadmap/module-system-1-module-language.md) — structures,
  signatures, transparent and opaque ascription, per-module type identity.
- [Stage 2 — Functors](roadmap/module-system-2-functors.md) — parametric modules with
  explicit application and sharing constraints.
- [Stage 3 — First-class modules](roadmap/module-system-3-first-class-modules.md) —
  modules as values; pack, unpack, dynamic module dispatch.
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
