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
system, expressions and parsing, and error handling. Two further design docs capture
cross-cutting work in flight: [design/module-system.md](design/module-system.md) — the
module-based abstraction system end-to-end (stage 1 shipped, remaining stages tracked
as `module-system-*` roadmap items below) — and [design/effects.md](design/effects.md)
— in-language monadic side effects (implementation tracked in
[roadmap/monadic-side-effects.md](roadmap/monadic-side-effects.md)). What's
shipped so far on the module-system and scheduler tracks: the dispatch-as-node
scheduler (every expression evaluates as a `Dispatch` node, so deferred work,
forward references, and cross-file references all reduce to the same
park-on-producer mechanism); the module-system stage 1 module language
(`MODULE` / `SIG` declarators, `:|` opaque and `:!` transparent ascription,
per-module type identity via `KType::ModuleType { scope_id, name }`, and
`Module` / `Signature` first-class values reachable via `Foo.member` ATTR
access); the dispatcher fold (overload resolution as one
`Scope::resolve_dispatch` chain walk returning a four-variant `ResolveOutcome`
whose `Resolved` carries the per-slot auto-wrap / replay-park / eager-sub
index buckets via `KFunction::classify_for_pick`); dispatch-time name
placeholders (binders install a `name → producer NodeId` entry in
`Scope::placeholders` at dispatch time so bare-identifier slot lookups whose
target binder has dispatched but not yet executed park on the producer instead
of failing with `UnboundName` — see [design/execution-model.md § Dispatch-time
name placeholders](design/execution-model.md#dispatch-time-name-placeholders));
the scheduler park-vs-own edge split (`DepEdge::Owned` / `DepEdge::Notify`
tagging so `free`'s recursive reclaim walks the ownership tree only and
ignores park edges installed by the single-Identifier short-circuit and
replay-park); and the eager-type-elaboration phase 1–3 slice plus the
parens-wrapped / phase-5 cleanup (one canonical runtime type representation,
scheduler-aware FN / STRUCT / UNION elaboration with self-recursive STRUCT
support and `LET T = T` cycle detection, FN parameter slots written
`(LIST_OF Number)` / `(DICT_OF Str Number)` scheduling a sub-Dispatch from
`parse_fn_param_list`, and the `NoopResolver` / `TypeResolver` /
`ScopeResolver` seam plus the legacy `parse_typed_field_list` deleted so
scope-aware elaboration goes exclusively through the scheduler-driven
`elaborate_type_expr`). The next signature revision after error handling lands
monadic side-effect capture; the type-system arc runs through the
module-system stages — foundation now landed in stage 1, ergonomic generic
dispatch in stage 5, coherence in stage 6.

## Next items

Items with no unresolved roadmap-level prerequisites — any of these can be picked up
without first landing something else:

- [Per-declaration type identity for structs and tagged unions](roadmap/per-declaration-type-identity.md)
  — extend the `KType::ModuleType` per-declaration identity carrier to `STRUCT` and
  `UNION` so two distinct declarations report distinct types; ships SCC
  pre-registration so mutually recursive STRUCT/UNION groups elaborate
  without deadlocking on each other's placeholders.
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
  ships standard effect modules (`Random`, `IO`, `Time`). Requires module-system
  stage 2's functor support so the `Wrap` slot can be higher-kinded.

### Module system

The agreed design is captured in [design/module-system.md](design/module-system.md);
stage 1 shipped (the module language: `MODULE`/`SIG` declarators, `:|`/`:!`
ascription, per-module type identity), and the remaining stages below land
the rest incrementally, each producing a usable end state.

- [Stage 2 — Module values and functors through the scheduler](roadmap/module-system-2-scheduler.md) —
  higher-kinded type slots (`KType::TypeConstructor`), sharing constraints
  (`<Type: E.Type>`), and the post-stage-1 Miri audit slate carry-forward.
  Requires eager type elaboration.
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
  along the lines of the module system's `KType::ModuleType` carrier, and
  ship SCC pre-registration on the same declaration surface so mutually
  recursive STRUCT/UNION groups elaborate without deadlocking.
- [Eager type elaboration with placeholder-based recursion](roadmap/eager-type-elaboration.md)
  — narrow remaining gaps: `OnceCell<KType>` late binding for signature-typed
  parameters whose type resolves only at functor application time (deferred
  pending a concrete case the parens-wrapped sub-Dispatch path doesn't cover),
  and the `KType::Unresolved` deletion (gated on a per-slot
  reference-vs-declaration opt-in in `classify_for_pick`, or a new `KObject`
  carrier preserving the surface `TypeExpr` through bind).
- [Chained type-binding LETs panic the scheduler](roadmap/chained-type-binding-let-panic.md)
  — `LET A = ... ; LET B = (... A ...) ; FN (... : B) -> ...` panics in
  `node_store.rs` with "result must be ready by the time it's read"; the bug
  is pre-existing and independent of the eager-type-elaboration parens-wrapped
  / phase-5 slice that surfaced it.

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

- [Static type checking and JIT compilation](roadmap/static-typing-and-jit.md) — the
  tooling and performance ceiling; both want a phase between parse and execution.
