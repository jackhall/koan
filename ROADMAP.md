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
runtime type system shipped — including `USING … SCOPE` block-scoped module opening, which
surfaces a module value's members as bare names for the duration of a block — with
implicit-search and axiom stages tracked as `module-system-*` roadmap items below). [design/effects.md](design/effects.md) captures the in-language
monadic side-effects design (tracked in [roadmap/libraries/monadic-side-effects.md](roadmap/libraries/monadic-side-effects.md)).

## Next items

Items with no unresolved roadmap-level prerequisites — any of these can be picked up
without first landing something else:

- [Files and imports](roadmap/libraries/files-and-imports.md) — wire `.koan` files together so
  a codebase can span more than one source file and files become modules.
- [Group-based operators](roadmap/libraries/group-based-operators.md) — paired `+`/`-`-style
  operators as a group; the syntax-level shorthand variant has no hard prerequisites.
- [Dependent parameter annotations](roadmap/type_language/dependent-param-annotations.md) —
  parameter type slots that reference earlier parameters in the same FN signature.
- [Structural KFunction admission across deferred return types](roadmap/type_language/kfunction-deferred-ret-precision.md)
  — per-call elaboration precision for structurally-typed FN slots.
- [Module and signature carriers move from KObject to KType](roadmap/type_language/module-signature-as-ktype.md)
  — collapse the KObject/KType duality for modules and signatures, retiring the dual-write
  in `KFunction::invoke` for signature-typed FN parameters.
- [VAL-slot value-carrier abstract-identity tagging](roadmap/type_language/val-slot-abstract-identity-tagging.md)
  — VAL-slot reads carry the SIG's abstract identity rather than the underlying value's
  concrete `KType`.
- [Generic value-slot binding via the destructuring unifier](roadmap/type_language/runtime-type-parameter-carriers.md)
  — runtime type-parameter carriers shipped; remaining is wiring `unify_slot` into invoke so
  generic value-slot FNs like `FN head (xs :(List T)) -> :T` become definable.
- [Lexical-order name resolution](roadmap/lexical-ordering.md) — make a name's visibility a
  function of its lexical position rather than the scheduler's queue order, so forward
  references resolve deterministically and sibling work can be reordered or parallelized.

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

- [Module and signature carriers move from KObject to KType](roadmap/type_language/module-signature-as-ktype.md)
- [FUNCTOR binder](roadmap/type_language/functor-binder.md)
- [Dependent parameter annotations](roadmap/type_language/dependent-param-annotations.md)
- [VAL-slot value-carrier abstract-identity tagging](roadmap/type_language/val-slot-abstract-identity-tagging.md)
- [Structural KFunction admission across deferred return types](roadmap/type_language/kfunction-deferred-ret-precision.md)
- [Generic value-slot binding via the destructuring unifier](roadmap/type_language/runtime-type-parameter-carriers.md)

### Editor tooling — [roadmap/editor_tooling/](roadmap/editor_tooling/)

Surface that lets external tools — editors, debuggers, build systems — see
intermediate Koan state. The build-time / run-time scheduler split is the
foundation:

- [Two-phase execution: build-time with pegged inputs, run-time resume](roadmap/editor_tooling/two-phase-execution.md)
- [Continue-on-error for the REPL and batch mode](roadmap/editor_tooling/continue-on-error.md)
