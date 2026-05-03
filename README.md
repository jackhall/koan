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

The builtins currently wired in are `LET <name> = <value>`, `PRINT <msg>`, `IF <predicate> THEN <value>`, and `FN <signature> -> <ReturnType> = <body>` — one file per builtin under [src/dispatch/builtins/](src/dispatch/builtins/), pulled together by [default_scope](src/dispatch/builtins.rs). Note: the scheduler eagerly evaluates every nested `(...)` before its parent dispatches, so `IF`/`THEN` is a post-hoc selector, not a lazy short-circuit.

User-defined functions declare a return type in the `-> Type` slot; the scheduler enforces it at runtime via `KErrorKind::TypeMismatch` when the body produces a value whose type doesn't match. `Any` is the no-op fast-path. The known types are `Number`, `Str`, `Bool`, `Null`, `List`, `Dict`, `KFunction`, `KExpression`, and `Any`.

Example:

```
LET x = 42
PRINT "hello"
FN (ECHO x) -> Number = (x)
LET y = (ECHO 21)
```

Indentation forms blocks (2-space increments, no tabs); `(` `)` group sub-expressions; `'…'` and `"…"` are string literals; numbers, `true`/`false`/`null` are literals. The lexer distinguishes three token classes for non-literal atoms: **all-caps tokens** (`LET`, `THEN`, `=`, `->`) are dispatch keywords; **capitalized names with at least one lowercase letter** (`Number`, `Str`, `KFunction`, `MyType`) are type references; everything else (lowercase / snake_case) is an identifier.

For a walk-through of the language surface with runnable snippets, see [TUTORIAL.md](TUTORIAL.md).

## Test

```sh
cargo test            # all unit tests
cargo test parse::    # tests under one module
```

Each module keeps its tests in a `#[cfg(test)] mod tests` block alongside the code (parser, scheduler, dispatch, and interpreter all have suites).

## Architecture

The pipeline is three stages, one per top-level module:

```
source ──▶ parse ──▶ dispatch ──▶ execute
        KExpression   KFuture      KObject
```

[src/main.rs](src/main.rs) wires them: read source, build a `default_scope` of builtins, hand the source to `interpret`.

### parse — text → `KExpression` tree

Entry point: `parse` in [src/parse/expression_tree.rs](src/parse/expression_tree.rs). The pipeline runs in passes:

1. [quotes.rs](src/parse/quotes.rs) — replace string-literal contents with placeholders so later passes don't re-tokenize them.
2. [whitespace.rs](src/parse/whitespace.rs) — turn indentation-based block structure into parenthesized form.
3. [expression_tree.rs](src/parse/expression_tree.rs) — walk the paren-delimited string into a nested expression tree.
4. [tokens.rs](src/parse/tokens.rs) — classify each whitespace-delimited token as a literal, keyword (no lowercase — `LET`, `=`, `THEN`, `->`), type name (capitalized + has lowercase — `Number`, `KFunction`), identifier, or compound (member access, indexing, prefix/suffix operators).
5. [operators.rs](src/parse/operators.rs) — table of compound-token operators (`!`, `.`, `[]`, `?`); add a row to extend.

The output is one [`KExpression`](src/parse/kexpression.rs) per top-level line: an ordered sequence of `ExpressionPart`s (`Keyword`, `Identifier`, `Type`, nested `Expression`, `ListLiteral`, or typed `Literal`). The `Keyword` vs slot split is the parser's contract with dispatch: only `Keyword` parts contribute fixed tokens to a signature's bucket key; `Identifier`, `Type`, literals, and sub-expressions all become slots that compete on type specificity.

### dispatch — `KExpression` → `KFuture` against a `Scope`

A [`Scope`](src/dispatch/scope.rs) is a lexical environment: parent link, name → value bindings, an indexed list of functions, and a pluggable output sink. `Scope::dispatch` scans registered functions for one whose [`ExpressionSignature`](src/dispatch/kfunction.rs) matches the incoming expression — signatures are an ordered mix of fixed `Token`s and typed `Argument` slots — then `bind`s the expression into a [`KFuture`](src/dispatch/scope.rs): the resolved function plus its `ArgumentBundle`, ready to run but not yet executed.

Runtime values are [`KObject`](src/dispatch/kobject.rs) (scalars, collections, expressions, futures, function references); cross-cutting traits (`Parseable`, `Executable`, `Serializable`, `Monadic`, …) live in [ktraits.rs](src/dispatch/ktraits.rs). Builtins are registered in [builtins.rs](src/dispatch/builtins.rs) and produce the default root scope.

Errors are first-class via [`KError`](src/dispatch/kerror.rs) — a `BodyResult::Err(KError)` arm propagates structured failures (type mismatches, unbound names, dispatch failures, shape errors) up the scheduler's Forward chain, accumulating call-stack frames as it walks. There is no in-language try/catch; errors short-circuit to the top level and the CLI formats them with frames. Future work adds in-language catch-as-builtin once the type system gains the necessary surface.

### execute — run the DAG

[`Scheduler`](src/execute/scheduler.rs) holds a directed acyclic graph of deferred work. Callers register pre-bound `KFuture`s via `add` / `add_with_deps`, or unbound `KExpression`s with `(part_index, dep)` substitutions via `add_pending` (each returned `NodeId` points backwards in submission order, so the graph is acyclic by construction). `execute` topologically sorts via Kahn's algorithm; for pending nodes it splices each dep's runtime result into the parent's parts as an `ExpressionPart::Future`, then dispatches and binds against the live scope before running.

[`interpret`](src/execute/interpret.rs) is the glue: parse the source, then walk each top-level expression post-order and submit every nested `(...)` to the scheduler — leaf expressions go in pre-bound, parents go in as pending with substitutions onto their sub-expressions' nodes. The caller keeps ownership of the `Scope` so output and post-run bindings are inspectable — that's how the tests in [interpret.rs](src/execute/interpret.rs) capture `PRINT` output and assert on `LET` bindings.

## Source layout

Inside [src/dispatch/](src/dispatch/), the `k`-prefix marks files built around a single
eponymous Koan-runtime type: [kobject.rs](src/dispatch/kobject.rs) defines `KObject`,
[kfunction.rs](src/dispatch/kfunction.rs) defines `KFunction`,
[kerror.rs](src/dispatch/kerror.rs) defines `KError`, [kkey.rs](src/dispatch/kkey.rs)
defines `KKey`, [ktraits.rs](src/dispatch/ktraits.rs) holds the `K*`-typed core traits.
Files without the prefix are infrastructure that don't introduce a single namesake type:
[arena.rs](src/dispatch/arena.rs) (allocation), [scope.rs](src/dispatch/scope.rs) (lexical
environment), [builtins.rs](src/dispatch/builtins.rs) (registry),
[monad.rs](src/dispatch/monad.rs) (trait impl on a foreign type),
[tagged_union.rs](src/dispatch/tagged_union.rs) (shared structure).

```
src/
├── main.rs              CLI entry point
├── parse.rs             pub mod parse; …
├── parse/
│   ├── kexpression.rs   parsed-expression types (KExpression, ExpressionPart, KLiteral)
│   ├── quotes.rs        mask string literals
│   ├── whitespace.rs    indentation → parens
│   ├── expression_tree.rs  build nested expressions; top-level parse()
│   ├── expression_tree_tests.rs  tests for expression_tree.rs and parse()
│   ├── dict_literal.rs  DictFrame state machine for `{k: v}` parsing
│   ├── tokens.rs        classify tokens, compound-operator desugaring
│   └── operators.rs     operator registry
├── dispatch.rs
├── dispatch/
│   ├── kobject.rs       runtime value type
│   ├── kerror.rs        KError, KErrorKind, Frame — structured runtime errors
│   ├── kfunction.rs     KFunction, signatures, ArgumentBundle, KType
│   ├── kkey.rs          KKey — hashable scalar wrapper for dict keys
│   ├── ktraits.rs       Parseable / Executable / Iterable / Serializable / Monadic
│   ├── arena.rs         RuntimeArena, CallArena — per-run and per-call allocation
│   ├── scope.rs         Scope and KFuture
│   ├── tagged_union.rs  shared tagged-union representation
│   ├── monad.rs         Monadic impl for Option
│   ├── builtins.rs      try_args!, register_builtin, default_scope()
│   └── builtins/        one file per builtin (body + register paired)
│       ├── let_binding.rs
│       ├── print.rs
│       ├── value_lookup.rs
│       ├── value_pass.rs
│       ├── if_then.rs
│       ├── fn_def.rs
│       ├── call_by_name.rs
│       ├── match_case.rs
│       ├── type_call.rs
│       └── union.rs
├── execute.rs
└── execute/
    ├── scheduler.rs     Scheduler struct, execute loop, KFunction::invoke bridge
    ├── nodes.rs         node types (NodeWork / NodeOutput / NodeStep / Node) + work_deps
    ├── run.rs           per-NodeWork-variant run_* methods (impl Scheduler)
    ├── lift.rs          lift_kobject — rebuild values across per-call arena boundaries
    ├── finalize.rs      finalize_ready_frames — promote forward-chain results out of
    │                    dying per-call arenas
    └── interpret.rs     parse → dispatch → schedule → execute
```

## Roadmap

Open structural items are tracked in [ROADMAP.md](ROADMAP.md); shipped ones are recorded
in [DECISIONS.md](DECISIONS.md).
