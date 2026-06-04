# Roadmap

Open structural items that don't fit in a single PR. Each entry below names the problem,
why it matters, and possible directions — not a fixed design. Per-item write-ups live in
this directory, one file per item.

The order matters. Sequencing is purely about technical and design dependencies — Koan has
no users yet, so backward-compatibility costs play no role. The cost being optimized is
engineering rework: doing one item before another it depends on means doing the dependent
item twice. Each per-item file ends with a **Dependencies** section linking to its
prerequisites and the items it unblocks.

Design rationale for what's already in the language lives in [design/](../design/) — five
topical docs covering the execution model, memory model, functional programming,
expressions and parsing, and error handling, plus [design/typing/](../design/typing/README.md)
covering the type and module systems end-to-end.

What's shipped that the open items below build on:

- *Operator-chain substrate.* Pure-symbol tokens that aren't builtin compound triggers
  classify as keywords, and [`KExpression`](../src/machine/model/ast.rs) caches a
  `DispatchShape` at parse time — including an `OperatorChain` track for the slot-led
  `Slot (Keyword Slot)+` shape, with its sorted-joined operator probe. A per-scope
  operator registry (`Bindings::operators`, walked by
  `Scope::resolve_operator_group_with_chain` like every other name) resolves a chain's
  probe to a shared `OperatorGroup`, and the `OperatorChain` dispatch arm hits that
  registry — missing cleanly on an undeclared or cross-group mix, or reaching the reduction
  seam on a hit. The reducer itself and the `GROUP`/`OP` declaration surface are the remaining
  open work under
  [user-definable n-ary operators](operator_chaining/n-ary-operators.md) and
  [user-defined operator modules](operator_chaining/user-defined-operator-modules.md).
  See [design/expressions-and-parsing.md § Structural cache and dispatch shape](../design/expressions-and-parsing.md#structural-cache-and-dispatch-shape).

## Next items

Items with no unresolved roadmap-level prerequisites — any of these can be picked up
without first landing something else:

- [Files and imports](libraries/files-and-imports.md) — wire `.koan` files together so
  a codebase can span more than one source file and files become modules.
- [Tagged-union variants as dispatchable types](type_language/tagged-variant-types.md) —
  promote each `UNION` variant to its own `KType` so `MATCH` collapses into type-dispatch.
- [Anonymous functions](anonymous-functions.md) — a keyword-less
  `FN {…}` literal that evaluates to a plain function value with no dispatch keyword.
- [Plain-English type-operation surfaces](type_language/type-operation-surfaces.md) —
  retire the `type_ops.rs` underscore keywords into the existing spaced / dotted / infix forms.
- [User-definable n-ary operators](operator_chaining/n-ary-operators.md) — the reduction pre-pass
  that lets a recognized operator run (`a + b + c`, `A | B | C`) evaluate by its group's mode.
- [Modular implicits (predicate-typing stage 5)](predicate_typing/modular-implicits.md) —
  implicit module resolution; its module-language substrate has already shipped.
- [Continue-on-error for the REPL and batch mode](editor_tooling/continue-on-error.md) —
  a top-level failure returns to the prompt and runs the next expression instead of ending the session.
- [Codebase-wide naming and responsibility audit](refactor/naming-and-responsibility-audit.md) —
  reconcile names with behavior across `src/**` (best sequenced after the in-flight type-language items).
- [Consolidate unsafe sites and prune the Miri slate](refactor/unsafe-consolidation.md) —
  funnel the scattered `unsafe` behind the arena boundary and drop slate tests that guard no distinct surface.
- [Seed every scope with builtins to skip the root walk](refactor/builtins-in-every-scope.md) —
  make builtins reachable at every scope so the hottest lookups stop walking the chain to root.

## Open items

Each subdirectory here is one project — a coherent body of work
whose items share design constraints and ship together. Per-item write-ups (problem,
impact, directions, dependencies) live in the subdirectory; the summaries below name
what the project buys the language and list its open items.

### Predicate typing — [predicate_typing/](predicate_typing/)

The user-facing typing stages — axioms, modular implicits, equivalence-checked
coherence, witness types — that ride on top of the type-language substrate.
The agreed design is captured in [design/typing/](../design/typing/README.md);
stages 1 and 2 shipped (the module language: `MODULE`/`SIG` declarators,
`:|`/`:!` ascription, per-module type identity, plus the scheduler-driven
elaborator, `SIG_WITH` sharing constraints, and higher-kinded type-constructor
slots, plus runtime type-parameter carriers on `List` / `Dict` / `Result`
values with ascription stamping at the FN return, argument, and `LET`
boundaries):

- [Stage 4 — Property testing and axioms](predicate_typing/axioms-and-generators.md)
- [Stage 5 — Modular implicits](predicate_typing/modular-implicits.md)
- [Stage 6 — Equivalence-checked coherence](predicate_typing/equivalence-checking.md)
- [Stage 7 — Syntax tuning and witness types](predicate_typing/syntax-tuning.md)

### Libraries — [libraries/](libraries/)

Give Koan a multi-file source surface, an in-language effect/error story, and
a canonical body of Koan code that exercises both. Each item is a piece of
substrate the standard library needs to exist as Koan source rather than as
Rust builtins:

- [Files and imports](libraries/files-and-imports.md)
- [Generalize `Scope::out` into monadic side-effect capture](libraries/monadic-side-effects.md)
- [Standard library](libraries/standard-library.md)

### Operator chaining — [operator_chaining/](operator_chaining/)

User-declarable operators and the n-ary chaining mechanism that evaluates them: a
recognized run of operators reduces by its group's declared mode — unary, fold, or
pairwise — and a module-scoped `GROUP`/`OP` surface populates the per-scope operator
registry the reducer walks.

- [User-definable n-ary operators](operator_chaining/n-ary-operators.md)
- [User-defined operator modules](operator_chaining/user-defined-operator-modules.md)

### Type language — [type_language/](type_language/)

Engine-level type-language substrate — how modules, signatures, functors,
deferred-return FNs, record-shaped parameter binding, and VAL-slot identity
are represented in `KType` and routed through dispatch. The substrate the
predicate-typing stages and the stdlib's functor-heavy collections both
build on:

- [Anonymous structural unions](type_language/anonymous-unions.md)
- [Tagged-union variants as dispatchable types](type_language/tagged-variant-types.md)
- [Plain-English type-operation surfaces](type_language/type-operation-surfaces.md)
- [Collapse `UserTypeKind` into a nominal-identity wrapper](type_language/nominal-identity-wrapper.md)

### Editor tooling — [editor_tooling/](editor_tooling/)

Surface that lets external tools — editors, debuggers, build systems — see
intermediate Koan state. The build-time / run-time scheduler split is the
foundation:

- [Two-phase execution: build-time with pegged inputs, run-time resume](editor_tooling/two-phase-execution.md)
- [Continue-on-error for the REPL and batch mode](editor_tooling/continue-on-error.md)

### Refactor — [refactor/](refactor/)

Cross-cutting cleanups that keep the engine legible and fast as it grows —
reconciling names with behavior, merging responsibilities that have drifted apart,
shrinking the unsafe surface, and cutting hot-path overhead:

- [Codebase-wide naming and responsibility audit](refactor/naming-and-responsibility-audit.md)
- [Consolidate unsafe sites and prune the Miri slate](refactor/unsafe-consolidation.md)
- [Seed every scope with builtins to skip the root walk](refactor/builtins-in-every-scope.md)
