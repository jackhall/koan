# Koan

A multiparadigm, graph-based language. Koan is in early prototype: it parses indentation-and-paren-structured source, dispatches each top-level expression against a registry of typed function signatures, and executes the resulting DAG of deferred calls.

## Build

Standard Cargo project, edition 2021.

```sh
cargo build           # debug build
cargo build --release # optimized build
```

The single binary target is `koan` (see [Cargo.toml](Cargo.toml)).

## Run

The CLI reads source from a file (first argument) or from stdin:

```sh
cargo run -- path/to/program.koan
echo 'PRINT "hello"' | cargo run
```

The builtins currently wired in are `LET <name> = <value>`, `PRINT <msg>`, `MATCH <value> WITH (<branches>)`, and `FN <signature> -> <ReturnType> = <body>` — one file per builtin under [src/builtins/](src/runtime/builtins), pulled together by [default_scope](src/runtime/builtins.rs). See [TUTORIAL.md](TUTORIAL.md) for the full builtin reference.

User-defined functions declare a return type in the `-> Type` slot; the scheduler enforces it at runtime via `KErrorKind::TypeMismatch` when the body produces a value whose type doesn't match. `Any` is the no-op fast-path. The surface-declarable types are `Number`, `Str`, `Bool`, `Null`, `:(List T)`, `:(Dict K V)`, `:(Function (args) -> R)`, `Type`, `Tagged`, `Struct`, `Module`, `Signature`, `KExpression`, and `Any`. Parameterized type expressions use the glued-right `:` sigil opening an S-expression group; bare types like `Number` and ascriptions like `x :Number` may write the sigil but don't require it on a non-parameterized atom.

Example:

```
LET x = 42
PRINT "hello"
FN (ECHO x :Number) -> Number = (x)
LET y = (ECHO 21)
```

Indentation forms blocks (2-space increments, no tabs); `(` `)` group sub-expressions; `'…'` and `"…"` are string literals; numbers, `true`/`false`/`null` are literals. The lexer distinguishes three token classes for non-literal atoms: **all-caps tokens** (`LET`, `THEN`, `=`, `->`) are dispatch keywords; **capitalized names with at least one lowercase letter** (`Number`, `Str`, `KExpression`, `MyType`) are type references; everything else (lowercase / snake_case) is an identifier.

For a walk-through of the language surface with runnable snippets, see [TUTORIAL.md](TUTORIAL.md).

## Test

```sh
cargo test            # all unit tests
cargo test parse::    # tests under one module
```

Each module keeps its tests in a `#[cfg(test)] mod tests` block alongside the code (parser, scheduler, dispatch, and interpreter all have suites). For the full testing and linting workflow — including the Miri audit slate that signs off the memory model under tree borrows — see [TEST.md](TEST.md).

## Architecture

The pipeline is three stages, split across two top-level modules:

```
source ──▶ parse ──▶ dispatch ──▶ execute
        KExpression   KFuture      KObject
```

`parse` is its own crate-top module; `dispatch` and `execute` live under `runtime` (alongside `runtime::builtins`). [src/main.rs](src/main.rs) wires the stages: read source, build a `default_scope` of builtins, hand the source to `interpret`.

### parse — text → `KExpression` tree

Entry point: `parse` in [src/parse/expression_tree.rs](src/parse/expression_tree.rs). The pipeline runs in passes:

1. [quotes.rs](src/parse/quotes.rs) — replace string-literal contents with placeholders so later passes don't re-tokenize them.
2. [whitespace.rs](src/parse/whitespace.rs) — turn indentation-based block structure into parenthesized form.
3. [expression_tree.rs](src/parse/expression_tree.rs) — walk the paren-delimited string into a nested expression tree.
4. [tokens.rs](src/parse/tokens.rs) — classify each whitespace-delimited token as a literal, keyword (pure-symbol like `=`, `->`, `:|`, or alphabetic with ≥2 uppercase letters and no lowercase — `LET`, `THEN`), type name (uppercase-leading with at least one lowercase — `Number`, `KFunction`, `IntOrd`), identifier, or compound (member access, indexing, prefix/suffix operators).
5. [operators.rs](src/parse/operators.rs) — table of compound-token operators (`!`, `.`, `[]`, `?`); add a row to extend.

The output is one [`KExpression`](src/runtime/machine/model/ast.rs) per top-level line: an ordered sequence of `ExpressionPart`s (`Keyword`, `Identifier`, `Type`, nested `Expression`, `ListLiteral`, or typed `Literal`). The `Keyword` vs slot split is the parser's contract with dispatch: only `Keyword` parts contribute fixed tokens to a signature's bucket key; `Identifier`, `Type`, literals, and sub-expressions all become slots that compete on type specificity.

### dispatch — `KExpression` → `KFuture` against a `Scope`

A [`Scope`](src/runtime/machine/core/scope.rs) is a lexical environment: parent link, name → value bindings, an indexed list of functions, and a pluggable output sink. `Scope::resolve_dispatch` walks the scope chain in a single pass and returns a [`ResolveOutcome`](src/runtime/machine/core/scope.rs) — `Resolved` (a unique pick, classified per slot), `Ambiguous(n)` (strict-mode tie), `Deferred` (no match yet but nested subs may unblock one), or `Unmatched` (a real dispatch failure). [`ExpressionSignature`](src/runtime/machine/model/types/signature.rs)s mix fixed `Token`s and typed `Argument` slots; on `Resolved` the scheduler `bind`s the resolved function into a [`KFuture`](src/runtime/machine/core/scope.rs) — the function plus its `ArgumentBundle`, ready to run but not yet executed.

Runtime values are [`KObject`](src/runtime/machine/model/values/kobject.rs) (scalars, collections, expressions, futures, function references); cross-cutting traits (`Parseable`, `Executable`, `Serializable`, `Monadic`, …) live in [ktraits.rs](src/runtime/machine/model/types/ktraits.rs). Builtins are registered in [builtins.rs](src/runtime/builtins.rs) and produce the default root scope.

Errors are first-class via [`KError`](src/runtime/machine/core/kerror.rs) — a `BodyResult::Err(KError)` arm propagates structured failures (type mismatches, unbound names, dispatch failures, shape errors) along the scheduler's dependency edges, accumulating call-stack frames as it walks. There is no in-language try/catch; errors short-circuit to the top level and the CLI formats them with frames. Future work adds in-language catch-as-builtin once the type system gains the necessary surface.

### execute — run the DAG

[`Scheduler`](src/runtime/machine/execute/scheduler.rs) holds a directed acyclic graph of deferred work. Callers register pre-bound `KFuture`s via `add` / `add_with_deps`, or unbound `KExpression`s with `(part_index, dep)` substitutions via `add_pending` (each returned `NodeId` points backwards in submission order, so the graph is acyclic by construction). `execute` topologically sorts via Kahn's algorithm; for pending nodes it splices each dep's runtime result into the parent's parts as an `ExpressionPart::Future`, then dispatches and binds against the live scope before running.

[`interpret`](src/runtime/machine/execute/interpret.rs) is the glue: parse the source, then walk each top-level expression post-order and submit every nested `(...)` to the scheduler — leaf expressions go in pre-bound, parents go in as pending with substitutions onto their sub-expressions' nodes. The caller keeps ownership of the `Scope` so output and post-run bindings are inspectable — that's how the tests in [interpret.rs](src/runtime/machine/execute/interpret.rs) capture `PRINT` output and assert on `LET` bindings.

## Source layout

The crate splits into two top-level modules: [parse](src/parse.rs) (text →
`KExpression`) and [runtime](src/runtime.rs) (everything that consumes a
`KExpression`). `runtime` divides into two siblings: [builtins/](src/runtime/builtins)
(the K-language standard library, one file per builtin) and
[machine/](src/runtime/machine) (the execution engine). `machine` further
splits into [model/](src/runtime/machine/model) (the value/type vocabulary —
[ast.rs](src/runtime/machine/model/ast.rs) for the parsed-expression types,
[types/](src/runtime/machine/model/types) for `KType`/signatures/traits, and
[values/](src/runtime/machine/model/values) for `KObject`/`KKey`/`Module`),
[core/](src/runtime/machine/core) (arenas, `Scope`, `KError`, plus the
`kfunction` submodule that owns `KFunction`/`Body`/`ArgumentBundle` — overload
resolution is one `Scope::resolve_dispatch` method that returns a
`ResolveOutcome`), and [execute/](src/runtime/machine/execute) (the scheduler
and the `interpret` glue).

Within those sub-modules, the `k`-prefix marks files built around a single
eponymous Koan-runtime type: [kobject.rs](src/runtime/machine/model/values/kobject.rs) defines `KObject`,
[kfunction.rs](src/runtime/machine/core/kfunction.rs) defines `KFunction`,
[kerror.rs](src/runtime/machine/core/kerror.rs) defines `KError`,
[kkey.rs](src/runtime/machine/model/values/kkey.rs) defines `KKey`,
[ktype.rs](src/runtime/machine/model/types/ktype.rs) defines `KType`,
[ktraits.rs](src/runtime/machine/model/types/ktraits.rs) holds the `K*`-typed core traits.
Files without the prefix are infrastructure that don't introduce a single namesake type:
[arena.rs](src/runtime/machine/core/arena.rs) (allocation),
[scope.rs](src/runtime/machine/core/scope.rs) (lexical environment plus the
`Scope::resolve_dispatch` overload-resolution walk and `Resolved` /
`ResolveOutcome` types),
[signature.rs](src/runtime/machine/model/types/signature.rs) (dispatch shapes and specificity,
including `ExpressionSignature::most_specific` for the per-bucket tournament),
[builtins.rs](src/runtime/builtins.rs) (registry),
[tagged_union.rs](src/runtime/builtins/tagged_union.rs) (shared structure),
[struct_value.rs](src/runtime/builtins/struct_value.rs) (shared structure),
[typed_field_list.rs](src/runtime/machine/model/types/typed_field_list.rs) (helper).

```
src/
├── main.rs              CLI entry point — re-imports through lib.rs
├── lib.rs               library facade — declares `parse` and `runtime` so integration tests under tests/ link against the same module graph
├── parse.rs             pub mod parse; …
├── parse/
│   ├── quotes.rs           mask string literals
│   ├── whitespace.rs       indentation → parens
│   ├── expression_tree.rs  build nested expressions; top-level parse()
│   ├── expression_tree_tests.rs  tests for expression_tree.rs and parse()
│   ├── dict_literal.rs     DictFrame state machine for `{k: v}` parsing
│   ├── frame.rs            Frame enum — per-paren-group parser sub-state
│   ├── parse_stack.rs      ParseStack — Frame stack with invariant-preserving methods
│   ├── triple_list.rs      helper for triple-list parsing
│   ├── type_expr_frame.rs  Frame::TypeExpr sub-state for `:(...)` type-expression groups
│   ├── tokens.rs           classify tokens, compound-operator desugaring
│   └── operators.rs        operator registry
├── runtime.rs           pub mod builtins / machine
└── runtime/
    ├── builtins.rs      try_args!, register_builtin, default_scope()
    ├── builtins/        one file per builtin (body + register paired)
    │   ├── let_binding.rs
    │   ├── print.rs
    │   ├── value_lookup.rs
    │   ├── value_pass.rs
    │   ├── attr.rs
    │   ├── fn_def.rs
    │   ├── fn_def/signature.rs   parameter-list parsing for FN
    │   ├── call_by_name.rs
    │   ├── cons.rs
    │   ├── match_case.rs
    │   ├── type_call.rs
    │   ├── type_ops.rs            LIST_OF / DICT_OF / FUNCTION_OF / MODULE_TYPE_OF
    │   ├── union.rs
    │   ├── struct_def.rs
    │   ├── struct_value.rs        shared struct-construction representation
    │   ├── tagged_union.rs        shared tagged-union representation
    │   ├── newtype_def.rs         NEWTYPE
    │   ├── module_def.rs          MODULE
    │   ├── sig_def.rs             SIG
    │   ├── val_decl.rs            VAL (SIG-body value-slot declarator)
    │   ├── ascribe.rs             :| / :! module ascription
    │   ├── test_support.rs
    │   ├── quote.rs               # surface form `#(expr)`
    │   └── eval.rs                # surface form `$(expr)`
    └── machine.rs       pub mod core / model / execute
        machine/
        ├── model.rs            re-exports from model::types and model::values
        ├── model/
        │   ├── ast.rs                 parsed-expression types (KExpression, ExpressionPart, KLiteral, TypeExpr)
        │   ├── types.rs
        │   ├── types/
        │   │   ├── ktype.rs           KType — type tag for slots, return types, and runtime values
        │   │   ├── ktype_predicates.rs   dispatch-time predicates (matches_value, accepts_part, is_more_specific_than)
        │   │   ├── ktype_resolution.rs   surface-name and TypeExpr elaboration (from_name, from_type_expr, join)
        │   │   ├── resolver.rs        Elaborator + elaborate_type_expr — scheduler-aware type-name elaboration with placeholder parking and per-scope resolution memo
        │   │   ├── signature.rs       ExpressionSignature, UntypedKey, Specificity — dispatch shape + tie-breaker
        │   │   ├── ktraits.rs         Parseable / Executable / Iterable / Serializable / Monadic
        │   │   └── typed_field_list.rs  shared parser for `(name :Type ...)` schemas
        │   ├── values.rs
        │   └── values/
        │       ├── kobject.rs         runtime value type
        │       ├── kkey.rs            KKey — hashable scalar wrapper for dict keys
        │       ├── named_pairs.rs     shared (name, value) ordered-list helper
        │       └── module.rs          Module / Signature — first-class module values
        ├── core.rs            module surface for core/
        ├── core/
        │   ├── arena.rs       RuntimeArena, CallArena — per-run and per-call allocation
        │   ├── bindings.rs    Bindings façade — four-map (data/functions/placeholders/types) with the validated try_apply write path and try_register_type for nominal type identity
        │   ├── kerror.rs      KError, KErrorKind, Frame — structured runtime errors
        │   ├── pending.rs     PendingQueue — deferred re-entrant writes, drained between dispatch nodes
        │   ├── scope.rs       Scope, KFuture, plus Scope::resolve_dispatch and the Resolved / ResolveOutcome types
        │   ├── scope_id.rs    ScopeId — counter-minted nominal scope identity for per-declaration types
        │   ├── kfunction.rs   KFunction, Body, BodyResult — body shapes plus the dispatch-to-execute bridge
        │   └── kfunction/
        │       ├── argument_bundle.rs   ArgumentBundle — resolved-slot carrier
        │       ├── body.rs
        │       ├── invoke.rs            KFunction::invoke — runtime side of the bind/apply pipeline
        │       └── scheduler_handle.rs
        ├── execute.rs
        └── execute/
            ├── scheduler.rs   Scheduler struct, execute loop, KFunction::invoke bridge; dep_graph/, node_store/, submit/, work_queues/, tests under it
            ├── nodes.rs       node types (NodeWork / NodeOutput / NodeStep / Node) + work_deps
            ├── run.rs         per-NodeWork-variant run_* methods (impl Scheduler); dispatch/, finish/, literal/ submodules
            ├── lift.rs        lift_kobject — rebuild values across per-call arena boundaries
            └── interpret.rs   parse → dispatch → schedule → execute
```

## Design and roadmap

Design rationale — one topical doc each. Mostly shipped behavior, but
sections may be aspirational where a decision has landed ahead of code.

- [design/execution-model.md](design/execution-model.md) — scheduler, deferred dispatch, per-call arenas.
- [design/memory-model.md](design/memory-model.md) — value ownership, lifting, lexical closures.
- [design/typing/](design/typing/README.md) — `KType`, dispatch by signature, structs and tagged
  unions, plus the module language (`MODULE`/`SIG`, ascription, functors, modular implicits,
  axiom-checked signatures, equivalence-checked coherence). Subdirectory because the type and
  module systems share the same scheduler-driven elaborator and nominal-identity carrier; the
  module language and `KType` runtime are shipped, with the implicit-search and axiom stages
  tracked under `roadmap/module-system-*.md`.
- [design/functional-programming.md](design/functional-programming.md) — function values, tail calls, signature-driven evaluation.
- [design/expressions-and-parsing.md](design/expressions-and-parsing.md) — the parse pipeline and `KExpression` shape.
- [design/error-handling.md](design/error-handling.md) — `KError`, propagation, and frame attribution.

[design/effects.md](design/effects.md) captures one further cross-cutting design ahead of
implementation: in-language monadic side effects — a `Monad` signature in Koan with concrete
effect modules (`Random`, `IO`, `Time`) ascribing it. Implementation is tracked in
[roadmap/monadic-side-effects.md](roadmap/monadic-side-effects.md).

Future work lives in [roadmap/](roadmap/) — one file per work item, with `Requires:` /
`Unblocks:` cross-links. [ROADMAP.md](ROADMAP.md) keeps the curated ordering and the
"Next items" grouping for picking up work.
