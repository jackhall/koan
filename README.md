# Koan

A multiparadigm, graph-based language with a metaprogrammable expression syntax and an ML-like module system.

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

The builtins currently wired in are `LET <name> = <value>`, `PRINT <msg>`, `MATCH <value> WITH (<branches>)`, `TRY (<expr>) WITH (<branches>)`, and `FN <signature> -> <ReturnType> = <body>` — one file per builtin under [src/builtins/](src/builtins), pulled together by [default_scope](src/builtins.rs). See [TUTORIAL.md](TUTORIAL.md) for the full builtin reference.

User-defined functions declare a return type in the `-> Type` slot; the scheduler enforces it at runtime via `KErrorKind::TypeMismatch` when the body produces a value whose type doesn't match. `Any` is the no-op fast-path. The surface-declarable types are `Number`, `Str`, `Bool`, `Null`, `:(LIST OF T)`, `:(MAP K -> V)`, `:(FN (args) -> R)`, `Type`, `Module`, `Signature`, `KExpression`, and `Any`; nominal types declared with `NEWTYPE`/`UNION` carry their own names. Parameterized type expressions use the glued-right `:` sigil opening an S-expression group; bare types like `Number` and ascriptions like `x :Number` may write the sigil but don't require it on a non-parameterized atom.

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

`parse`, `builtins`, and `machine` are sibling crate-top modules; `machine` owns dispatch and execute. [src/main.rs](src/main.rs) reads the source and hands it to `interpret_with_writer_path`, which builds a `default_scope` of builtins and drains the scheduler.

### parse — text → `KExpression` tree

Entry point: `parse` in [src/parse/expression_tree.rs](src/parse/expression_tree.rs). The pipeline runs in passes:

1. [quotes.rs](src/parse/quotes.rs) — replace string-literal contents with placeholders so later passes don't re-tokenize them.
2. [whitespace.rs](src/parse/whitespace.rs) — turn indentation-based block structure into parenthesized form.
3. [expression_tree.rs](src/parse/expression_tree.rs) — walk the paren-delimited string into a nested expression tree.
4. [tokens.rs](src/parse/tokens.rs) — classify each whitespace-delimited token as a literal, keyword (pure-symbol like `=`, `->`, `:|`, or alphabetic with ≥2 uppercase letters and no lowercase — `LET`, `THEN`), type name (uppercase-leading with at least one lowercase — `Number`, `KFunction`, `IntOrd`), identifier, or compound (member access, indexing, prefix/suffix operators).
5. [operators.rs](src/parse/operators.rs) — table of compound-token operators (`!`, `.`, `[]`, `?`); add a row to extend.

The output is one [`KExpression`](src/machine/model/ast.rs) per top-level line: an ordered sequence of `ExpressionPart`s (`Keyword`, `Identifier`, `Type`, nested `Expression`, `ListLiteral`, or typed `Literal`). The `Keyword` vs slot split is the parser's contract with dispatch: only `Keyword` parts contribute fixed tokens to a signature's bucket key; `Identifier`, `Type`, literals, and sub-expressions all become slots that compete on type specificity.

### dispatch — `KExpression` → `KFuture` against a `Scope`

A [`Scope`](src/machine/core/scope.rs) is a lexical environment: parent link, name → value bindings, an indexed list of functions, and a pluggable output sink. [`resolve_dispatch`](src/machine/execute/dispatch/resolve_dispatch.rs) walks the scope chain in a single pass and returns a [`ResolveOutcome`](src/machine/execute/dispatch/resolve_dispatch.rs) — `Resolved` (a unique pick, classified per slot), `Ambiguous(n)` (strict-mode tie), `Deferred` (no match yet but nested subs may unblock one), or `Unmatched` (a real dispatch failure). [`ExpressionSignature`](src/machine/model/types/signature.rs)s mix fixed `Token`s and typed `Argument` slots; on `Resolved` the resolved function binds its arguments, ready to run but not yet executed.

Runtime values are [`KObject`](src/machine/model/values/kobject.rs) (scalars, collections, expressions, futures, function references); cross-cutting traits (`Parseable`, `Executable`, `Serializable`, `Monadic`, …) live in [ktraits.rs](src/machine/model/types/ktraits.rs). Builtins are registered in [builtins.rs](src/builtins.rs) and produce the default root scope.

Errors are first-class via [`KError`](src/machine/core/kerror.rs) — a `Done(Err(KError))` outcome propagates structured failures (type mismatches, unbound names, dispatch failures, shape errors) along the scheduler's dependency edges, accumulating call-stack frames as it walks. `TRY (<expr>) WITH (<branches>)` catches in-language; uncaught errors short-circuit to the top level and the CLI formats them with frames. See [design/error-handling.md](design/error-handling.md) for the per-arm `it` shape and the privilege boundary that keeps builtin and user errors disjoint.

### execute — run the DAG

[`Scheduler`](src/machine/execute/scheduler.rs) holds a slot table of in-flight work plus a push/notify dependency graph; [`KoanHarness`](src/machine/execute/harness.rs) owns it and is the sole holder of `&mut Scheduler`. Callers submit a top-level block via the harness's `enter_block` (and nested parts via `add_dispatch`); each slot's decide spawns sub-Dispatches for the expression's nested parts and parks the parent as a `Bind` until its deps terminalize. When a producer writes its terminal, a single `finalize` step drains the producer's notify-list and wakes any consumer whose `pending_deps` counter hits zero — no polling, no result-table sweep. Tail returns (an `Action::Tail` lowered to `Outcome::Continue`) rewrite the slot's own work in place rather than allocating a new slot. See [design/execution-model.md](design/execution-model.md).

[`interpret`](src/machine/execute/interpret.rs) is the glue: parse the source, hand the top-level block to `enter_block` against a root `default_scope`, drain the scheduler, then `read_result` each top-level node. `PRINT` output flows through the scope's pluggable writer (default stdout; tests swap in a shared `Vec<u8>` buffer to read it back), and every value the program allocated dies with the per-run `RuntimeArena` when `interpret` returns.

## Source layout

The crate splits into three top-level modules: [parse](src/parse.rs) (text →
`KExpression`), [builtins/](src/builtins) (the K-language standard library, one
file per builtin), and [machine/](src/machine) (the execution engine that
consumes a `KExpression`). `machine` further
splits into [model/](src/machine/model) (the value/type vocabulary —
[ast.rs](src/machine/model/ast.rs) for the parsed-expression types,
[types/](src/machine/model/types) for `KType`/`KKind`/signatures/traits, and
[values/](src/machine/model/values) for `KObject`/`Carried`/`KKey`/`Module`),
[core/](src/machine/core) (arenas, `Scope`, `KError`, plus the
`kfunction` submodule that owns `KFunction`/`Body` and the body executor), and
[execute/](src/machine/execute) (the scheduler, the `dispatch` shape router —
where overload resolution lives as `resolve_dispatch` returning a
`ResolveOutcome` — and the `interpret` glue).

Within those sub-modules, the `k`-prefix marks files built around a single
eponymous Koan-runtime type: [kobject.rs](src/machine/model/values/kobject.rs) defines `KObject`,
[kfunction.rs](src/machine/core/kfunction.rs) defines `KFunction`,
[kerror.rs](src/machine/core/kerror.rs) defines `KError`,
[kkey.rs](src/machine/model/values/kkey.rs) defines `KKey`,
[ktype.rs](src/machine/model/types/ktype.rs) defines `KType`,
[ktraits.rs](src/machine/model/types/ktraits.rs) holds the `K*`-typed core traits.
Files without the prefix are infrastructure that don't introduce a single namesake type:
[arena.rs](src/machine/core/arena.rs) (allocation),
[scope.rs](src/machine/core/scope.rs) (lexical environment and `KFuture`),
[resolve_dispatch.rs](src/machine/execute/dispatch/resolve_dispatch.rs) (the
overload-resolution walk returning a `ResolveOutcome`),
[signature.rs](src/machine/model/types/signature.rs) (dispatch shapes and specificity),
[recursive_set.rs](src/machine/model/types/recursive_set.rs) (`RecursiveSet`, the
`Rc`-owned unit of nominal identity, allocation, and lift),
[builtins.rs](src/builtins.rs) (registry),
[tagged_union.rs](src/machine/execute/dispatch/constructors/tagged_union.rs) (shared structure),
[typed_field_list.rs](src/machine/model/types/typed_field_list.rs) (helper).

```
src/
├── main.rs              CLI entry point — reads source, calls interpret_with_writer_path
├── lib.rs               library facade — declares `parse`, `builtins`, and `machine` so integration tests under tests/ link against the same module graph
├── parse.rs             pub mod parse; …
├── parse/
│   ├── quotes.rs           mask string literals
│   ├── whitespace.rs       indentation → parens
│   ├── expression_tree.rs  build nested expressions; top-level parse()
│   ├── dict_literal.rs     DictFrame state machine for `{k: v}` parsing
│   ├── frame.rs            Frame enum — per-paren-group parser sub-state
│   ├── parse_stack.rs      ParseStack — Frame stack with invariant-preserving methods
│   ├── triple_list.rs      helper for triple-list parsing
│   ├── tokens.rs           classify tokens, compound-operator desugaring
│   └── operators.rs        operator registry
├── builtins.rs          register_builtin, default_scope()
├── builtins/            one file per builtin (body + register paired)
│   ├── let_binding.rs
│   ├── print.rs
│   ├── attr.rs
│   ├── fn_def.rs             FN — user function definition
│   ├── fn_def/signature.rs      parameter-list parsing for FN
│   ├── fn_def/return_type.rs    return-type slot elaboration
│   ├── fn_def/param_refs.rs     parameter-reference resolution
│   ├── fn_def/finalize.rs       seal the function once its slots resolve
│   ├── match_case.rs
│   ├── try_with.rs           TRY (<expr>) WITH (<branches>) — catch runtime errors
│   ├── catch.rs              CATCH — error-handling primitive
│   ├── branch_walk.rs        shared <tag> -> <body> walker for MATCH and TRY
│   ├── result.rs             Result tagged-union builtin
│   ├── type_constructors.rs  keyworded type-language overloads (LIST OF / MAP _ -> _ / FN / FUNCTOR)
│   ├── type_ops.rs           TEMPLATE / WITH
│   ├── type_ops/type_constructor.rs   TEMPLATE — parameterized type constructor
│   ├── type_ops/with.rs               WITH — type-constructor application
│   ├── union.rs              UNION — tagged-union declaration
│   ├── record_projection.rs  FROM — `(x y) FROM r` re-tags a record value's carried type to the named fields
│   ├── nominal_schema.rs     shared Action-harness field-list elaboration for UNION / NEWTYPE record repr
│   ├── newtype_def.rs        NEWTYPE — scalar repr and the `:{…}` record repr (the product-side nominal form)
│   ├── recursive_types.rs    RECURSIVE TYPES — co-declare a mutually-recursive nominal group
│   ├── module_def.rs         MODULE
│   ├── sig_def.rs            SIG
│   ├── functor_def.rs        FUNCTOR — modules parameterized by modules
│   ├── val_decl.rs           VAL (SIG-body value-slot declarator)
│   ├── ascribe.rs            :| / :! module ascription
│   ├── using_scope.rs        USING — lexical-scope introduction
│   ├── test_support.rs
│   ├── quote.rs              # surface form `#(expr)`
│   └── eval.rs               # surface form `$(expr)`
├── machine.rs           pub mod core / model / execute
└── machine/
    ├── model.rs            re-exports from model::types and model::values
    ├── model/
    │   ├── ast.rs                 parsed-expression types (KExpression, ExpressionPart, KLiteral, TypeName); classify_dispatch_shape
    │   ├── operators.rs           OperatorGroup registry record — chainable-operator precedence/associativity
    │   ├── types.rs
    │   ├── types/
    │   │   ├── ktype.rs           KType — type tag for slots, return types, and runtime values
    │   │   ├── kkind.rs           KKind — the shallow dispatch *kind* of a type (the OfKind expectation)
    │   │   ├── record.rs          Record<V> — ordered identifier-keyed map backing struct schemas and FN/FUNCTOR parameter identity
    │   │   ├── ktype_predicates.rs   dispatch-time predicates (matches_value, accepts_part, is_more_specific_than)
    │   │   ├── ktype_resolution.rs   surface-name and TypeName elaboration (from_name, from_type_expr, join)
    │   │   ├── resolver.rs        Elaborator + elaborate_type_expr — scheduler-aware type-name elaboration with placeholder parking and per-scope resolution memo
    │   │   ├── recursive_set.rs   RecursiveSet — Rc-owned unit of nominal identity, allocation, and lift
    │   │   ├── signature.rs       ExpressionSignature, UntypedKey, Specificity — dispatch shape + tie-breaker
    │   │   ├── ktraits.rs         Parseable / Executable / Iterable / Serializable / Monadic
    │   │   └── typed_field_list.rs  shared parser for `(name :Type ...)` schemas
    │   ├── values.rs
    │   └── values/
    │       ├── kobject.rs         runtime value type
    │       ├── carried.rs         Carried — the scheduler's value currency (Object | Type)
    │       ├── kkey.rs            KKey — hashable scalar wrapper for dict keys
    │       ├── named_pairs.rs     shared (name, value) ordered-list helper
    │       └── module.rs          Module / Signature — first-class module values
    ├── core.rs            module surface for core/
    ├── core/
    │   ├── arena.rs       RuntimeArena, CallArena — per-run and per-call allocation
    │   ├── bindings.rs    Bindings façade — five-map (data/functions/placeholders/types/pending_overloads) with the validated try_apply write path, try_register_type for nominal type identity, and the visibility-aware lookup_value/lookup_type/lookup_function surface (raw map accessors are #[cfg(test)])
    │   ├── bindings/pending.rs   per-binding pending-overload state
    │   ├── kerror.rs      KError, KErrorKind, Frame — structured runtime errors
    │   ├── pending.rs     PendingQueue — deferred re-entrant writes, drained between dispatch nodes
    │   ├── scope.rs       Scope, KFuture — lexical environment and dispatch-result handle
    │   ├── scope_ptr.rs   ScopePtr — the single audited owner of Scope lifetime-erasure for arena-stored carriers
    │   ├── source.rs      source-span and provenance carrier for errors
    │   ├── scope_id.rs    ScopeId — counter-minted nominal scope identity for per-declaration types
    │   ├── lexical_frame.rs  LexicalFrame — immutable cactus-chain (scope_id, index, parent) attached to every dispatched node
    │   ├── kfunction.rs   KFunction, Body — body shapes plus the dispatch-to-execute bridge
    │   └── kfunction/
    │       ├── body.rs              Body / ReturnContract
    │       ├── bind_by_name.rs      bind a user call's resolved args to params by name
    │       ├── exec.rs              run_user_fn — innermost body executor; returns a scheduler-unaware ExecOutcome
    │       ├── action.rs            Action — the scheduler-aware currency a builtin returns (types only)
    │       ├── pick.rs              per-bucket tournament selecting the most-specific overload
    │       └── scheduler_handle.rs  NodeId — stable DAG node handle
    ├── execute.rs
    └── execute/
        ├── scheduler.rs   Scheduler struct — read views + inherent write primitives (the AST-free store the harness drives); dep_graph/, node_store/, submit/, work_queues/, finish/ (run_wait — one node handler), execute/ (the pop loop), splice/ (bare-name forward alias) submodules, tests under it
        ├── nodes.rs       node types (NodeWork struct / NodeOutput / NodeStep / Node) + work_deps
        ├── outcome.rs     Outcome — the unified scheduler-step currency (Done / Continue / ParkThenContinue / Invoke / Redispatch / Forward) + Continuation + cont combinators (short_circuit / catch_cont / ignore_results); AST-free (carries DepRequest as an opaque type)
        ├── harness.rs     KoanHarness — owns the Scheduler, the sole &mut holder: the execute loop, apply_outcome (sole graph writer), the AST-aware submission wrappers, submit_dispatch, literal lowering; plus run_action (lowers a builtin Action to an Outcome, pure)
        ├── dispatch.rs    classify_dispatch (the decide) + decide/decide_with_presubs + classify_dispatch_shape; submit/ (binder-aware submit_dispatch chokepoint), literal/ (aggregate-literal lowering), ctx/ (SchedulerView read view), exec/ (dispatch-side invoke), keyworded/, fn_value/, single_poll/, head_deferred/, apply_callable/, operator_chain/, field_list/, constructors/, resolve_dispatch/, resolve_type_expr/ submodules
        ├── lift.rs        lift_kobject — rebuild values across per-call arena boundaries
        └── interpret.rs   parse → enter_block → execute
```

## Design and roadmap

Design rationale — one topical doc each. Mostly shipped behavior, but
sections may be aspirational where a decision has landed ahead of code.
[design/README.md](design/README.md) is the design-tree index — what
each doc owns, the foundation-vs-seam heuristic the refactor analysis
uses, and pointers to the analysis tooling.

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
[roadmap/monadic-side-effects.md](roadmap/libraries/monadic-side-effects.md).

Future work lives in [roadmap/](roadmap/) — one file per work item, with `Requires:` /
`Unblocks:` cross-links. Its [README](roadmap/README.md) curates the open items by project
and derives a "Next items" list — everything with no still-open prerequisite — from those
cross-links (`tools/doclinks.py sync-next`).
