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
expressions and parsing, and error handling, plus [design/typing/](design/typing/README.md)
covering the type and module systems end-to-end.

What's shipped that the open items below build on:

- *Module language.* `MODULE` / `SIG` declarators, `:|` / `:!` ascription, `SIG_WITH`
  sharing constraints, higher-kinded type-constructor slots, and the type-language
  collapse that puts modules and signatures in `KType` directly via `KType::Module`,
  `KType::Signature`, and `KType::AbstractType` carriers. Values carry runtime
  type-parameter carriers, stamped at FN return, argument, and `LET` boundaries.
- *Block-scoped module opening.* `USING … SCOPE` surfaces a module value's members as
  bare names for the duration of a block, splitting reads and writes across the
  transparent-scope `outer` chain.
- *FUNCTOR binder.* A dedicated `FUNCTOR` binder with its `:(Functor (params) -> R)`
  type-position sigil and the one-way `KFunctor` / `KFunction` admissibility wall.
- *Effects design.* [design/effects.md](design/effects.md) captures the in-language
  monadic side-effects design (tracked in
  [roadmap/libraries/monadic-side-effects.md](roadmap/libraries/monadic-side-effects.md)).
- *Lexical-provenance chain.* Every dispatched node carries an immutable cactus-chain
  `LexicalFrame { scope_id, index, parent }` attached at block entry; top-level,
  `MODULE`, `SIG`, FN-body, and MATCH / TRY arm submissions all funnel through one
  `Scheduler::enter_block` primitive, and each MATCH / TRY arm is its own lexical
  block — closing the divergent-bind hazard structurally and giving the remaining
  dispatch-fix phases a queue-order-independent provenance signal to read from.
- *Index-gated name resolution.* `Scope::resolve_with_chain` and the function-bucket
  `OverloadBucket::pick` filter every hit through the `idx < cutoff` visibility
  predicate (with a `nominal_binder` carve-out for `STRUCT` / named `UNION` / `SIG` /
  `FUNCTOR` / `MODULE`), so forward references resolve by lexical position rather
  than by queue arrival order and `UnboundName` becomes structural rather than
  transient.
- *Recursive binder submission.* `Scheduler::add_with_chain` walks each binder-shaped
  Dispatch's eager Expression-slot parts and submits them as sub-Dispatches at the
  same outermost submission point, so nested binders' placeholders all install before
  any sibling can dispatch. The pre-submitted children ride through `NodeWork::Dispatch.pre_subs`
  into Phase 4, which reuses them instead of allocating fresh sub-Dispatches.
- *Visibility-aware `Bindings` lookups.* Production reads go through
  `Bindings::lookup_value` / `lookup_type` / `lookup_function`, each taking a
  `chain_cutoff: Option<usize>` and applying the per-entry visibility predicate
  inside the lookup. `lookup_function` returns a
  `FunctionLookup::{Bucket, Pending, None}` shape pre-filtered for per-overload
  visibility and folds the bucket / `pending_overloads` fall-through into the
  single dispatch ancestor walk. The five raw `RefCell` map accessors
  (`data` / `types` / `functions` / `placeholders` / `pending_overloads`) are
  gated `#[cfg(test)]`; production sites that legitimately sweep all members
  (module surface mirroring, signature shape-check, REPL reflection) use the
  value-yielding `iter_data` / `iter_types` / `iter_functions`.
- *Type language via dispatch.* The `:(...)` sigil is a parse-context marker
  emitting `ExpressionPart::SigiledTypeExpr(Box<KExpression>)` with no inner
  shape-folding; the dispatcher's `SigiledTypeExpr` fast lane tail-replaces
  the slot with a `Dispatch` of the wrapped expression. Keyworded
  overloads — `LIST OF`, `MAP _ -> _`, `FN`, `FUNCTOR` — register in
  `builtins/type_constructors.rs` and serve every fresh parameterized-type
  annotation. The submission walk reifies the binder install channel as
  `BinderKey::Name` (`LET` / `STRUCT` / `UNION` / `SIG` / `MODULE`) vs.
  `BinderKey::Bucket` (`FN` / `FUNCTOR`), and `pending_overloads` carries a
  per-bucket Vec so sibling FN / FUNCTOR overloads coexist as distinct
  wake sources with earliest-index-visible parking.

## Next items

Items with no unresolved roadmap-level prerequisites — any of these can be picked up
without first landing something else:

- [Files and imports](roadmap/libraries/files-and-imports.md) — wire `.koan` files together so
  a codebase can span more than one source file and files become modules.
- [Group-based operators](roadmap/libraries/group-based-operators.md) — paired `+`/`-`-style
  operators as a group; the syntax-level shorthand variant has no hard prerequisites.
- [Per-call type-parameter binding in parameter signatures](roadmap/type_language/type-parameter-binding.md)
  — free type-parameter names in parameter slots bind per call, from either an
  argument's carried type structure or an earlier parameter's value.
- [Branch-arm return-type agreement](roadmap/branch-arm-return-type.md) — give MATCH and
  TRY a static return type (arms-agree vs synthesized-union vs hybrid), closing the
  divergent-result hazard symmetric to the divergent-bind hazard the lexical-provenance
  phase closes structurally.
- [RETURN from anywhere](roadmap/early-return.md) — explicit `(RETURN <expr>)` form
  that ends the enclosing FN's body from any position and TCO-optimizes when `<expr>`
  is a function call, decoupling tail-call position from "last statement in the body".

## Open items

Each subdirectory of [roadmap/](roadmap/) is one project — a coherent body of work
whose items share design constraints and ship together. Per-item write-ups (problem,
impact, directions, dependencies) live in the subdirectory; the summaries below name
what the project buys the language and list its open items.

### Predicate typing — [roadmap/predicate_typing/](roadmap/predicate_typing/)

The user-facing typing stages — axioms, modular implicits, equivalence-checked
coherence, witness types — that ride on top of the type-language substrate.
The agreed design is captured in [design/typing/](design/typing/README.md);
stages 1 and 2 shipped (the module language: `MODULE`/`SIG` declarators,
`:|`/`:!` ascription, per-module type identity, plus the scheduler-driven
elaborator, `SIG_WITH` sharing constraints, and higher-kinded type-constructor
slots, plus runtime type-parameter carriers on `List` / `Dict` / `Result`
values with ascription stamping at the FN return, argument, and `LET`
boundaries):

- [Stage 4 — Property testing and axioms](roadmap/predicate_typing/axioms-and-generators.md)
- [Stage 5 — Modular implicits](roadmap/predicate_typing/modular-implicits.md)
- [Stage 6 — Equivalence-checked coherence](roadmap/predicate_typing/equivalence-checking.md)
- [Stage 7 — Syntax tuning and witness types](roadmap/predicate_typing/syntax-tuning.md)

### Libraries — [roadmap/libraries/](roadmap/libraries/)

Give Koan a multi-file source surface, an in-language effect/error story, and
a canonical body of Koan code that exercises both. Each item is a piece of
substrate the standard library needs to exist as Koan source rather than as
Rust builtins:

- [Files and imports](roadmap/libraries/files-and-imports.md)
- [Generalize `Scope::out` into monadic side-effect capture](roadmap/libraries/monadic-side-effects.md)
- [Group-based operators](roadmap/libraries/group-based-operators.md)
- [Standard library](roadmap/libraries/standard-library.md)

### Type language — [roadmap/type_language/](roadmap/type_language/)

Engine-level type-language substrate — how modules, signatures, functors,
deferred-return FNs, dependent parameter annotations, generic value-slot
binding, and VAL-slot identity are represented in `KType` and routed through
dispatch. The substrate the predicate-typing stages and the stdlib's
functor-heavy collections both build on:

- [Per-call type-parameter binding in parameter signatures](roadmap/type_language/type-parameter-binding.md)
- [VAL-slot ATTR re-tagging](roadmap/type_language/val-slot-attr-retagging.md)
- [Structural KFunction admission across deferred parameter and return slots](roadmap/type_language/kfunction-deferred-ret-precision.md)
- [FN/FUNCTOR named identity](roadmap/type_language/fn-named-identity.md)

### Dispatch fix — [roadmap/dispatch_fix/](roadmap/dispatch_fix/)

Untangle dispatch into queue-order-independent name resolution plus a single
unified ancestor walk per call site. The provenance-plumbing, index-gated
resolution, recursive-binder-submission, and type-language-via-dispatch
phases have shipped (see "What's shipped so far"); the remaining items pick
up the SCC-context gap surfaced by routing the type language through the
dispatcher, the user-functor application surface, and the walk-unification
collapse:

- [Unified walk + strict-only admission](roadmap/dispatch_fix/unified-walk.md)
- [SCC-aware dispatcher for parameterized self-recursive types](roadmap/dispatch_fix/scc-aware-dispatcher-for-self-recursive-types.md)
- [User-defined TypeConstructor keyworded application](roadmap/dispatch_fix/user-defined-typeconstructor-keyworded-application.md)

### Editor tooling — [roadmap/editor_tooling/](roadmap/editor_tooling/)

Surface that lets external tools — editors, debuggers, build systems — see
intermediate Koan state. The build-time / run-time scheduler split is the
foundation:

- [Two-phase execution: build-time with pegged inputs, run-time resume](roadmap/editor_tooling/two-phase-execution.md)
- [Continue-on-error for the REPL and batch mode](roadmap/editor_tooling/continue-on-error.md)
