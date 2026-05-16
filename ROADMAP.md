# Roadmap

Open structural items that don't fit in a single PR. Each entry below names the problem,
why it matters, and possible directions — not a fixed design. Per-item write-ups live in
[roadmap/](roadmap/).

The order matters. Sequencing is purely about technical and design dependencies — Koan has
no users yet, so backward-compatibility costs play no role. The cost being optimized is
engineering rework: doing one item before another it depends on means doing the dependent
item twice. Each per-item file ends with a **Dependencies** section linking to its
prerequisites and the items it unblocks.

Design rationale for what's already in the language lives in [design/](design/) — five
topical docs covering the execution model, memory model, functional programming,
expressions and parsing, and error handling, plus the [design/typing/](design/typing/README.md)
subdirectory covering the type and module systems end-to-end (the module language and
runtime type system shipped, implicit-search and axiom stages tracked as `module-system-*`
roadmap items below). [design/effects.md](design/effects.md) captures the in-language
monadic side-effects design (tracked in [roadmap/monadic-side-effects.md](roadmap/monadic-side-effects.md)).

## Next items

Items with no unresolved roadmap-level prerequisites — any of these can be picked up
without first landing something else:

- [Files and imports](roadmap/files-and-imports.md) — wire `.koan` files together so a
  codebase can span more than one source file and files become modules.
- [Simplify `runtime::machine` and shrink AI context cost](roadmap/simplify-and-shrink-context.md)
  — `runtime::machine` owns ~60% of the crate's fractal coupling index and three
  non-test files exceed 600 lines; score reshuffles via `modgraph_rewrite.py`,
  split the largest files, then trim scheduler tests the sub-struct extractions
  made redundant.

## Open items

### Memory and runtime substrate

- [Generalize `Scope::out` into monadic side-effect capture](roadmap/monadic-side-effects.md)
  — replace the ad-hoc `Box<dyn Write>` with an in-language `Monad` signature
  (see [design/effects.md](design/effects.md)) plus a runtime `Effectful<T>` carrier;
  ships standard effect modules (`Random`, `IO`, `Time`). The `Wrap` slot's
  higher-kinded surface (`(TYPE_CONSTRUCTOR Type)`) has landed via module-system
  stage 2.

### Module system

The agreed design is captured in [design/typing/](design/typing/README.md);
stages 1 and 2 shipped (the module language: `MODULE`/`SIG` declarators,
`:|`/`:!` ascription, per-module type identity, plus the scheduler-driven
elaborator, `SIG_WITH` sharing constraints, higher-kinded
type-constructor slots, and the post-stage-1 Miri audit-slate
carry-forward), and the remaining stages below land the rest
incrementally, each producing a usable end state.

- [Dependent parameter annotations](roadmap/module-system-dependent-param-annotations.md) —
  parameter type slots that reference earlier parameters in the same FN
  signature (`(MAKE T: Type elt: T)`, OCaml's
  `module Make (E : ORDERED) (S : SET with type elt = E.t)`). Reuses
  the `ReturnType` / `DeferredReturn` carrier shipped at
  [`ExpressionSignature::return_type`](src/machine/model/types/signature.rs)
  and the per-call re-elaboration plumbing in
  [`KFunction::invoke`](src/machine/core/kfunction/invoke.rs); the new
  work is staged left-to-right dispatch.
- [VAL-slot value-carrier abstract-identity tagging](roadmap/val-slot-abstract-identity-tagging.md)
  — a value read from an `:|`-ascribed module's VAL-declared slot today
  carries the underlying value's `KType`, not the per-call abstract
  identity `:|` minted for the SIG's `Type` member; closes the
  deferred end-to-end functor-on-VAL-slot call test variant in
  [`functor_return_module_type_of_parameter_resolves_per_call`](src/builtins/fn_def/tests/module_stage2.rs)
  and aligns dispatch keys for stage 5's implicit search over
  VAL-typed values.
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
- [Structural KFunction admission across deferred return types](roadmap/kfunction-deferred-ret-precision.md) —
  [`function_value_ktype`](src/machine/model/values/kobject.rs) synthesizes
  `KType::KFunction { ret: KType::Any }` for deferred-return FNs because the
  structural function-type language has no surface for "per-call
  elaboration of this expression"; the symmetric coarsening in
  [`function_compat`](src/machine/model/types/ktype_predicates.rs) admits-or-
  rejects-by-`==` so today's strict refusal stays safe but silent. A
  `debug_assert!` at the coarsening branch is the tripwire; the decision
  is forced when stage 5 implicit search or a precise FN-typed slot
  ascription first exercises the scenario.

### Surface and ergonomics

- [Files and imports](roadmap/files-and-imports.md) — a Koan codebase is one file;
  no way for a `.koan` file to reach into another, and no story for how files become
  modules.
- [Error-handling surface follow-ups](roadmap/error-handling.md) — errors-as-values,
  source spans on `KExpression`, continue-on-error (independent), plus typed
  user errors and the catch surface (gated on module-system stage 2).
- [Standard library](roadmap/standard-library.md) — collections (`Set`, `Map`,
  …) and standard effect modules (`Random`, `IO`, `Time`) ship as Koan-source
  functor FNs across multiple `.koan` files; doubles as the canonical example
  of idiomatic module / signature / functor / import composition.

### Future-facing

- [Two-phase execution: build-time with pegged inputs, run-time resume](roadmap/two-phase-execution.md) —
  pre-run error surfacing and the performance ceiling, both falling out of
  the same pegged-frontier scheduler run plus stalled-DAG snapshot.
