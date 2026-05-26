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
surfaces a module value's members as bare names for the duration of a block, and the
type-language collapse that puts modules and signatures in `KType` directly via
`KType::Module { .. }` / `KType::Signature(_)` / `KType::AbstractType { .. }` carriers,
retiring the `KObject::KModule` / `KObject::KSignature` duality — with
implicit-search and axiom stages tracked as `module-system-*` roadmap items below). [design/effects.md](design/effects.md) captures the in-language
monadic side-effects design (tracked in [roadmap/libraries/monadic-side-effects.md](roadmap/libraries/monadic-side-effects.md)).
The dispatch driver's wrap-slot rail collapsed too: bare-name parts now eager-resolve
against the dispatching scope and splice their carrier into the slot in place, retiring the
old `apply_auto_wrap` + sub-Dispatch detour and unifying the Identifier-LHS and Type-LHS
self-cycle surfaces under `SchedulerDeadlock`. The dedicated `FUNCTOR` binder shipped
alongside its `:(Functor (params) -> R)` type-position sigil, the one-way
`KFunctor`/`KFunction` admissibility wall, and the Type-class `LET` allowlist flip
(`KTypeValue` ∪ nominal-identity ∪ `is_functor`-flagged `KFunction`) that closes the
plain-function-bound-to-a-Type-class-name hole. The FUNCTOR-return validator
folded into `classify_return_type` itself, so the carrier is walked once for
both Resolved/Deferred classification and the admissibility verdict. FUNCTOR
application now reads naturally — `(MAKESET IntOrd)` works directly when
`IntOrd` is a Type-classified module satisfying the declared signature — via
two pieces: a value-side `LET` partition guard that rejects module/signature
RHSes bound under lowercase names (forcing module/signature carriers onto
Type-classified identifiers, so the type-side binding map is the single home
for them), and a bucket-keyed `pending_overloads` dispatch park that lets a
bare-arg call to a still-finalizing FN / FUNCTOR overload park on the binder
slot instead of racing FIFO submission order into `DispatchFailed`. The
`:Type` parameter slot admits any `KTypeValue`-carried type — bare builtin
tokens (`Number`, `Str`, `Bool`, `Null`) along with `TaggedUnionType` /
`StructType` schema carriers — so single-type-parameter functor surfaces
like `(MAKETREE Number)` work directly without a signature-typed wrapper
module per builtin, while module and signature carriers still route through
their dedicated slots to preserve the overload wall.

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
- [Lexical provenance plumbing](roadmap/dispatch_fix/lexical-provenance.md) — first phase
  of the dispatch-fix project: attach an immutable cactus-chain frame
  `{ scope_id, index, parent }` to every unit of work and route top-level / FN body /
  MODULE body through a single `enter_block` primitive, plumbing the data the
  index-gated resolver later reads.

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

### Dispatch fix — [roadmap/dispatch_fix/](roadmap/dispatch_fix/)

Untangle dispatch into queue-order-independent name resolution plus a single
unified ancestor walk per call site. Phases land sequentially: provenance
plumbing first, then the index-gated `Resolution` split that makes visibility
lexical, then a structural fix to the nested-binder submission race, then the
walk-unification and strict-only admission collapse:

- [Lexical provenance plumbing](roadmap/dispatch_fix/lexical-provenance.md)
- [Index-gated resolution](roadmap/dispatch_fix/index-gated-resolution.md)
- [Nested-binder recursive submission](roadmap/dispatch_fix/nested-binder-submission.md)
- [Unified walk + strict-only admission](roadmap/dispatch_fix/unified-walk.md)

### Editor tooling — [roadmap/editor_tooling/](roadmap/editor_tooling/)

Surface that lets external tools — editors, debuggers, build systems — see
intermediate Koan state. The build-time / run-time scheduler split is the
foundation:

- [Two-phase execution: build-time with pegged inputs, run-time resume](roadmap/editor_tooling/two-phase-execution.md)
- [Continue-on-error for the REPL and batch mode](roadmap/editor_tooling/continue-on-error.md)
