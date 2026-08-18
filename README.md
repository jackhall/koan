# Koan

A functional, graph-based language with a metaprogrammable expression syntax and an ML-like module system.

## Build

Standard Cargo project, edition 2024.

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

The builtins wired into the default scope include `LET`, `PRINT`, and `FN`; the nominal-type declarators `UNION` and `NEWTYPE`; the control forms `MATCH <value> -> :<Type> WITH (<branches>)`, `TRY (<expr>) -> :<Type> WITH (<branches>)`, and `CATCH`; the module forms `MODULE`, `SIG`, `USING`, the `:!` / `:|` ascription operators, and `TYPE OF <value>` (a value's own type — a module's is its signature); the arithmetic and comparison operators `+ - * / < <= > >=` and `AND`, and the type-union operator `|` building `:(A | B)` (chained runs like `1 < 2 < 3` or `A | B | C` reduce per their operator group's mode — see [expressions and parsing](design/expressions-and-parsing.md)); the operator declarators `OP` and `GROUP`, with which a module declares its own chainable operators (see [operators](design/operators.md)); and the `#` / `$` quote and eval sigils — one file per builtin under [src/builtins/](src/builtins), pulled together by [seed_builtins](src/builtins.rs). See the [tutorial](tutorial/README.md) for a feature-by-feature walkthrough, and [tutorial/reference.md](tutorial/reference.md) for a one-page surface reference.

User-defined functions declare a return type in the `-> Type` slot; the scheduler enforces it at runtime via `KErrorKind::TypeMismatch` when the body produces a value whose type doesn't match. `Any` is the no-op fast-path. The surface-declarable types are `Number`, `Str`, `Bool`, `Null`, `:(LIST OF Elem)`, `:(MAP Key -> Val)`, `:(FN (args) -> Out)`, `Type`, `Module`, `Signature`, `KExpression`, and `Any`; nominal types declared with `NEWTYPE`/`UNION` carry their own names. Parameterized type expressions use the glued-right `:` sigil opening an S-expression group; bare types like `Number` and ascriptions like `x :Number` may write the sigil but don't require it on a non-parameterized atom.

Example:

```
LET x = 42
PRINT "hello"
FN (ECHO x :Number) -> Number = (x)
LET y = (ECHO 21)
```

Indentation forms blocks (2-space increments, no tabs); `(` `)` group sub-expressions; `'…'` and `"…"` are string literals; numbers, `true`/`false`/`null` are literals. The lexer sorts non-literal atoms into three classes: **keywords** — pure-symbol tokens (`=`, `->`) or alphabetic tokens with ≥2 uppercase letters and no lowercase (`LET`, `THEN`) — are dispatch markers; **type references** are uppercase-leading with at least one lowercase letter (`Number`, `Str`, `KExpression`, `MyType`); everything else (lowercase / snake_case) is an identifier. An uppercase-leading token that fits neither shape (a lone capital, or all-caps-with-digits) is a parse error.

For a walk-through of the language surface with runnable snippets, see the [tutorial](tutorial/README.md).

## Test

```sh
cargo test            # all unit tests
cargo test parse::    # tests under one module
```

Each module keeps its tests in a `#[cfg(test)] mod tests` block alongside the code (parser, scheduler, dispatch, and interpreter all have suites). For the full testing and linting workflow — including the Miri audit slate that signs off the memory model under tree borrows — see [TEST.md](TEST.md).

Measurement scaffolding that no build ships — the counting global allocator, the debug reach-tightness report, and the recorded koan programs the allocation baselines are read over — lives outside `src/` under [audit/](audit/README.md), which carries the charter for the split plus the committed baseline table and the script that reproduces it.

## Architecture

The pipeline is three stages, split across two top-level modules:

```
source ──▶ parse ──▶ dispatch ──▶ execute
        KExpression  DispatchOutcome  KObject
```

`parse`, `builtins`, and `machine` are sibling crate-top modules; `machine` owns dispatch and execute. [src/main.rs](src/main.rs) reads the source and hands it to `interpret_with_writer_path`, which stands up the scope pair, seeds the builtins, and drains the scheduler.

### parse — text → `KExpression` tree

Entry point: `parse` in [src/parse/expression_tree.rs](src/parse/expression_tree.rs). The pipeline runs in passes:

1. [quotes.rs](src/parse/quotes.rs) — replace string-literal contents with placeholders so later passes don't re-tokenize them.
2. [whitespace.rs](src/parse/whitespace.rs) — turn indentation-based block structure into parenthesized form.
3. [expression_tree.rs](src/parse/expression_tree.rs) — walk the paren-delimited string into a nested expression tree.
4. [tokens.rs](src/parse/tokens.rs) — classify each whitespace-delimited token as a literal, keyword (pure-symbol like `=`, `->`, `:|`, or alphabetic with ≥2 uppercase letters and no lowercase — `LET`, `THEN`), type name (uppercase-leading with at least one lowercase — `Number`, `KFunction`, `Ordered`), identifier, or compound (member access, indexing, suffix operators).
5. [operators.rs](src/parse/operators.rs) — table of compound-token operators (`.`, `[]`, `?`); add a row to extend.

The output is one [`KExpression`](src/machine/model/ast.rs) per top-level line: an ordered sequence of `ExpressionPart`s (`Keyword`, `Identifier`, `Type`, nested `Expression`, `ListLiteral`, or typed `Literal`). The `Keyword` vs slot split is the parser's contract with dispatch: only `Keyword` parts contribute fixed tokens to a signature's bucket key; `Identifier`, `Type`, literals, and sub-expressions all become slots that compete on type specificity.

`KExpression` is a `Copy` handle: its parts run and every string in it borrow the program storage the parse bumped them into. The scheduler dispatches a separate [`WorkingExpression`](src/machine/model/ast/working.rs), which is where a resolved sub-result gets spliced back in — so an expression *value* can never carry one. A node only reaches the value channel wrapped in the [program-storage marker](src/machine/model/ast/program.rs), which types the tier the channel's verdicts assume. See [design/expressions-and-parsing.md](design/expressions-and-parsing.md).

### dispatch — `KExpression` → `DispatchOutcome` against a `Scope`

A [`Scope`](src/machine/core/scope.rs) is a lexical environment: parent link, name → value bindings, an indexed list of functions, and a pluggable output sink. [`resolve_dispatch`](src/machine/execute/decide/resolve_dispatch.rs) walks the scope chain in a single pass and returns a [`DispatchOutcome`](src/machine/execute/decide/resolve_dispatch.rs) — `Resolved` (a unique pick, classified per slot), `Ambiguous(n)` (strict-mode tie), `Deferred` (no match yet but nested subs may unblock one), `ParkOnProducers` (wait on a still-finalizing earlier binder), `UnboundName`, or `Unmatched` (a real dispatch failure). [`ExpressionSignature`](src/machine/model/types/signature.rs)s mix fixed `Token`s and typed `Argument` slots; on `Resolved` the resolved function binds its arguments, ready to run but not yet executed.

Runtime values are [`KObject`](src/machine/model/values/kobject.rs) (scalars, collections, expressions, function references); the cross-cutting `Parseable` trait lives in [ktraits.rs](src/machine/model/types/ktraits.rs). Builtins are registered in [builtins.rs](src/builtins.rs) and produce the default root scope.

Errors are first-class via [`KError`](src/machine/core/kerror.rs) — a `Done(Err(KError))` outcome propagates structured failures (type mismatches, unbound names, dispatch failures, shape errors) along the scheduler's dependency edges, accumulating call-stack frames as it walks. `TRY (<expr>) WITH (<branches>)` catches in-language; uncaught errors short-circuit to the top level and the CLI formats them with frames. See [design/error-handling.md](design/error-handling.md) for the per-arm `it` shape and the privilege boundary that keeps builtin and user errors disjoint.

### execute — run the DAG

The [`Scheduler`](workgraph/src/scheduler.rs) — the [workgraph](workgraph/README.md) crate's — holds a slot table of in-flight work plus a push/notify dependency graph over first-class edges, and its `drain` owns the pop loop; [`KoanRuntime`](src/machine/execute/harness.rs) owns the scheduler beside the koan-side `Host` whose `step` is the drain callback. Callers submit a top-level block via the harness's `enter_block`; each slot's decide spawns sub-Dispatches for the expression's nested parts and parks the parent as a dep-finish until its deps terminalize. When a producer finalizes, a single walk delivers its terminal into every waiting edge's destination region and wakes any consumer whose pending count hits zero — no polling, no result-table sweep, and the producer's slot reclaims behind the walk. Tail returns (an `Action::Tail` lowered to `Outcome::Continue`) rewrite the slot's own work in place rather than allocating a new slot. See [the execution model](design/execution/README.md).

[`interpret`](src/machine/execute/interpret.rs) is the glue: parse the source, allocate the run-root scope and its `RunScope` child (`unseeded_scopes`), establish the run frame, seed the builtins against that frame's type registry (`seed_builtins`), hand the top-level block to `enter_block`, drain the scheduler, then `read_result` each top-level node. `PRINT` output flows through the scope's pluggable writer (default stdout; tests swap in a shared `Vec<u8>` buffer to read it back), and every value the program allocated dies with the per-run `KoanRegion` when `interpret` returns.

## Source layout

The crate splits into three top-level modules: [parse](src/parse.rs) (text →
`KExpression`), [builtins/](src/builtins) (the K-language standard library, one
file per builtin), and [machine/](src/machine) (the execution engine that
consumes a `KExpression`). `machine` further
splits into [model/](src/machine/model) (the value/type vocabulary —
[ast.rs](src/machine/model/ast.rs) for the parsed-expression types,
[types/](src/machine/model/types) for `KType`/`KKind`/signatures/traits, and
[values/](src/machine/model/values) for `KObject`/`Carried`/`KKey`/`Module`),
[core/](src/machine/core) (allocation, `Scope`, `KError`, plus the
`kfunction` submodule that owns `KFunction`/`Body` and the body executor), and
[execute/](src/machine/execute) (the drain harness, the `decide` shape router —
where overload resolution lives as `resolve_dispatch` returning a
`DispatchOutcome` — and the `interpret` glue).

Within those sub-modules, the `k`-prefix marks files built around a single
eponymous Koan-runtime type: [kobject.rs](src/machine/model/values/kobject.rs) defines `KObject`,
[kfunction.rs](src/machine/core/kfunction.rs) defines `KFunction`,
[kerror.rs](src/machine/core/kerror.rs) defines `KError`,
[kkey.rs](src/machine/model/values/kkey.rs) defines `KKey`,
[ktype.rs](src/machine/model/types/ktype.rs) defines `KType`,
[ktraits.rs](src/machine/model/types/ktraits.rs) holds the `K*`-typed core traits.
Files without the prefix are infrastructure that don't introduce a single namesake type:
[arena.rs](src/machine/core/arena.rs) (allocation),
[scope.rs](src/machine/core/scope.rs) (lexical environment),
[resolve_dispatch.rs](src/machine/execute/decide/resolve_dispatch.rs) (the
overload-resolution walk returning a `DispatchOutcome`),
[signature.rs](src/machine/model/types/signature.rs) (dispatch shapes and specificity),
[node.rs](src/machine/model/types/node.rs) (`TypeNode`, one interned type's content —
the thing a `KType` handle names),
[recursive_group_window.rs](src/machine/model/types/recursive_group_window.rs) (the
declarator-local pre-seal window a co-declared nominal group elaborates against, and
the SCC seal that interns its members),
[declaration_window.rs](src/machine/model/types/declaration_window.rs) (the ambient
window a module body's announced type declarations elaborate against, plus the two
views every consult path shares),
[type_digest.rs](src/machine/model/types/type_digest.rs) (`TypeDigest`, the eager
content-hash every `KType` compares by),
[sig_schema.rs](src/machine/model/types/sig_schema.rs) (`SigSchema`, the owned
schema a signature node carries, and the canonical signature-subtyping relation),
[registry.rs](src/machine/model/types/registry.rs) (`TypeRegistry`, the
run-frame-owned store that memoizes subtype verdicts by digest pair),
[builtins.rs](src/builtins.rs) (registry),
[constructors.rs](src/machine/execute/decide/constructors.rs) (shared structure),
[typed_field_list.rs](src/machine/model/types/typed_field_list.rs) (helper).

```
src/
├── main.rs              CLI entry point — reads source, calls interpret_with_writer_path
├── lib.rs               library facade — declares `parse`, `builtins`, and `machine` so integration tests under tests/ link against the same module graph
├── tests.rs             `#[cfg(test)]` crate-wide test scaffolding — installs audit/'s counting global allocator for the lib-test binary and exposes the tally fixed-cost measurements read
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
├── builtins.rs          register_builtin, unseeded_scopes(), seed_builtins()
├── builtins/            one file per builtin (body + register paired)
│   ├── let_binding.rs
│   ├── print.rs
│   ├── attr.rs
│   ├── fn_def.rs             FN — user function definition
│   ├── fn_def/signature.rs      parameter-list parsing for FN
│   ├── fn_def/return_type.rs    return-type slot elaboration
│   ├── fn_def/param_refs.rs     parameter-reference resolution
│   ├── fn_def/finalize.rs       seal the function once its slots resolve
│   ├── match_case.rs         MATCH — branch by the scrutinee's runtime type
│   ├── try_with.rs           TRY (<expr>) WITH (<branches>) — catch runtime errors
│   ├── catch.rs              CATCH — error-handling primitive
│   ├── branch_walk.rs        MATCH's by-type arm walker + TRY's by-tag walker + shared arm-tail machinery
│   ├── result.rs             Result tagged-union builtin
│   ├── parameterized_types.rs  keyworded type-language overloads (LIST OF / MAP _ -> _ / FN)
│   ├── type_ops.rs           WITH — infix signature specialization; TYPE OF — value → type
│   ├── type_ops/with.rs               WITH — abstract-slot pinning + manifest fixity
│   ├── type_ops/type_of.rs            TYPE OF — a value's own type (a module's is its signature)
│   ├── union.rs              UNION — sum-type declaration (dissolves to one newtype per variant, joined by an anonymous union)
│   ├── type_union.rs         `|` — the `:(A | B)` anonymous-union type constructor
│   ├── record_projection.rs  FROM — `(x y) FROM r` re-tags a record value's carried type to the named fields
│   ├── nominal_schema.rs     shared Action-harness field-list elaboration for UNION / NEWTYPE record repr
│   ├── newtype_def.rs        NEWTYPE — scalar repr, the `:{…}` record repr, and the `(Param… AS Name)` constructor-family mint
│   ├── module_def.rs         MODULE — including the body's type-declaration announcement, which co-declares a mutually-recursive nominal group
│   ├── op_def.rs             OP / UNARY OP — declare a chainable operator over an operand type
│   ├── group_def.rs          GROUP — a module bundling mutually chainable operators under one reduction mode
│   ├── sig_def.rs            SIG
│   ├── val_decl.rs           VAL (SIG-body value-slot declarator)
│   ├── type_decl.rs          TYPE — SIG-body abstract type-member declarators (bare + higher-kinded)
│   ├── ascribe.rs            :| / :! module ascription
│   ├── using_scope.rs        USING — lexical-scope introduction
│   ├── test_support.rs
│   └── eval.rs               # surface form `$(expr)`
├── machine.rs           pub mod core / model / execute
└── machine/
    ├── model.rs            re-exports from model::types and model::values
    ├── model/
    │   ├── ast.rs                 raw parsed AST (KExpression, ExpressionPart, KLiteral, TypeIdentifier) — Copy handles over bumped slices
    │   ├── ast/
    │   │   ├── shape.rs           Part / PartClass / FieldSlot + the structural readers both part families share (classify_dispatch_shape, the bucket key, the operator probe)
    │   │   └── working.rs         WorkingExpression / WorkingPart — the scheduler's own node, the only one that can hold a spliced sub-result
    │   ├── operators.rs           OperatorGroup registry record — chainable-operator precedence/associativity
    │   ├── types.rs
    │   ├── types/
    │   │   ├── ktype.rs           KType — the Copy content-digest handle for slots, return types, and runtime values
    │   │   ├── kkind.rs           KKind — the shallow dispatch *kind* of a type (the OfKind expectation)
    │   │   ├── node.rs            TypeNode — one interned type's content, the thing a KType handle names
    │   │   ├── registry.rs        TypeRegistry — the run-frame-owned interning graph and verdict cache
    │   │   ├── record.rs          Record<V> — ordered identifier-keyed map backing record-type schemas and FN parameter identity
    │   │   ├── ktype_predicates.rs   dispatch-time predicates (matches_value, accepts_part, is_more_specific_than)
    │   │   ├── ktype_resolution.rs   surface-name and TypeName elaboration (from_name, from_type_expr, join)
    │   │   ├── resolver.rs        Elaborator + elaborate_type_expr — scheduler-aware type-name elaboration with placeholder parking (no cache tier; interning already makes a re-elaborated form yield the same handle)
    │   │   ├── recursive_group_window.rs   RecursiveGroupWindow — the pre-seal group window and the SCC seal that interns its members
    │   │   ├── sig_schema.rs      SigSchema + sig_subtype — a signature type's owned schema and the subtyping relation
    │   │   ├── signature.rs       ExpressionSignature, UntypedKey, Specificity — dispatch shape + tie-breaker
    │   │   ├── ktraits.rs         Parseable / Serializable
    │   │   └── typed_field_list.rs  shared parser for `(name :Type ...)` schemas
    │   ├── values.rs
    │   └── values/
    │       ├── kobject.rs         runtime value type
    │       ├── container_substrate.rs  ContainerSubstrate<'a, C> — the index-generic region-resident substrate (sectioned cells + run union + copy cost), Copy and bump-hosted in every arm; C is RecordLayout (a sorted name slice), a dict's frozen &BumpBackedMap, or a list/payload marker
    │       ├── carried.rs         Carried — the scheduler's value currency (Object | Type)
    │       ├── kkey.rs            KKey — hashable scalar wrapper for dict keys
    │       ├── named_pairs.rs     shared (name, value) ordered-list helper
    │       └── module.rs          Module — first-class module values and their sealed self-sig content
    ├── core.rs            module surface for core/
    ├── core/
    │   ├── arena.rs       KoanRegion (= Region<KoanStorageProfile>), RegionBrand, FoldingBrand, KoanRegionExt — the Koan storage substrate and allocation veneer (children below)
    │   ├── arena/
    │   │   ├── frame.rs           FrameStorage / FrameSet / CallFrame / RunWriter — per-call allocation frame, run-root storage, the run's output sink, witnessed child-scope construction door
    │   │   └── step_allocator.rs  StepAllocator — the step-branded construction doors (alloc_carried / alloc_type_* / alloc_object_scalar)
    │   ├── bindings.rs    Bindings façade — four-map (data/types/functions/operators), each slot bound or claimed by a still-finalizing binder (ValueSlot/TypeSlot/OverloadSlot), with the firm write_value / write_type / write_operator_group primitives, the visibility-aware lookup_value/lookup_type/lookup_function surface (raw map accessors are #[cfg(test)]); one RefCell over all four maps, nothing else interior-mutable
    │   ├── bindings/
    │   │   ├── ops.rs     WriteOp / TypeWritePolicy — a binding-table write as outcome data, and the single apply interpreter the run loop drives
    │   │   └── gate.rs    WriteGate — the zero-sized capability every table write verb requires, minted only inside crate::machine (run loop + unpublished-scope construction door)
    │   ├── kerror.rs      KError, KErrorKind, TraceFrame — structured runtime errors
    │   ├── scope.rs       Scope — lexical environment: the bump-resident struct, its allocators (alloc_run_root / alloc_child_under / … , bumped at 'a; alloc_child_transparent through the crossing born door) with their private constructors, and small accessors (children below)
    │   ├── scope/
    │   │   ├── resolve.rs     name-resolution ladders — value / type / operator-group lookup, walk_chain / resolve_builtin_first, visibility cutoff, builtin-shadow consults
    │   │   ├── registry.rs    write doors — the seal_* construction halves of the value binds, the submission-channel placeholder installs, the owns-its-bindings write-target guard, and the *_direct writes for unpublished scopes
    │   │   └── reach.rs       reach / carrier derivation — resident value / type carriers, envelope sealing, copy-free / copying adoption, and the module store folds
    │   ├── ref_carriers.rs  ScopeRefFamily / ModuleRefFamily — the Reattachable families a region-stored &Scope / &Module carrier erases through
    │   ├── source.rs      source-span and provenance carrier for errors
    │   ├── scope_id.rs    ScopeId — counter-minted nominal scope identity for per-declaration types
    │   ├── statement_id.rs  StatementId — counter-minted, never-recycled identity of one submitted statement; what a binding entry's Installer names, so declaration identity borrows nothing from the scheduler
    │   ├── lexical_frame.rs  LexicalFrame — immutable cactus-chain (scope_id, index, parent) attached to every dispatched node
    │   ├── kfunction.rs   KFunction, Body — body shapes plus the dispatch-to-execute bridge
    │   └── kfunction/
    │       ├── body.rs              Body / ReturnContract
    │       ├── exec.rs              run_user_fn — innermost body executor; returns a scheduler-unaware ExecOutcome
    │       ├── action.rs            Action — the scheduler-aware currency a builtin returns: the WriteOp effects it decided plus its ActionKind continuation (types only)
    │       ├── block_tail.rs        the one "run a block, return the tail" constructor — the sole Action::Tail site, configured by EVAL / MATCH / TRY arms / USING
    │       └── pick.rs              per-bucket tournament selecting the most-specific overload
    ├── execute.rs
    └── execute/
        ├── harness.rs     KoanRuntime (Scheduler + Host side by side) — Host::step is the drain callback (open the sealed continuation at one rank-2 step brand, decide, drain binding writes), Host::apply is the sole &mut Scheduler code (wire_deps, the Outcome → StepVerdict map), plus run_action (lowers a builtin Action to an Outcome, pure), run_program, and the AST-aware submission wrappers (enter_block / dispatch_in_scope / dispatch_in_own_scope / dispatch_body / submit_dep_finish_witnessed_in_own_scope)
        ├── interpret.rs   the embedder API: the interpret → interpret_with_writer → interpret_with_writer_path ladder
        ├── nodes.rs       node types: the NodeWork re-export from the scheduler, plus SlotFrame / NodeScope / NodePayload / ChainOp
        ├── producer_id.rs  ProducerId — the opaque park token everything below the drive loop stores, compares, and hands back but cannot open (both conversions pub(in crate::machine::execute)), plus deps_on / extend_deps_on, the single verb pair that spends one
        ├── outcome.rs     Outcome — the unified scheduler-step currency (Done / Continue / Park / Forward) + Continuation (Finish / Catch / Resume) + the Await envelope builder (sole finish-carrying-Park constructor) + cont combinators (short_circuit / catch_cont / ignore_results); AST-free (carries DepRequest as an opaque type)
        ├── ambient.rs     AmbientContext — the per-step ambient state (active frame, run frame, slot payload, declared-return obligation)
        ├── decide.rs      classify_dispatch (the decide) + decide_tail + classify_dispatch_shape; submit/ (binder-aware submit_expression chokepoint), literal/ (aggregate-literal lowering), ctx/ (DecideCtx — the scheduler-free step context), resolve/ (Resolution — THE bare-name ladder), exec/ (decide-side invoke), keyworded/, fn_value/, single_poll/, head_deferred/, apply_callable/, operator_chain/, field_list/, constructors/, resolve_dispatch/, resolve_type_identifier/ submodules
        └── lift.rs        lift_kobject — rebuild values across per-call region boundaries
```

## Design and roadmap

Design rationale lives under [design/](design/README.md) — one topical doc per
concern, describing shipped behavior, with sections that run ahead of code where
a decision has landed early. [design/](design/README.md) is the index:
what each doc owns, the foundation-vs-seam heuristic the refactor analysis uses,
and pointers to the analysis tooling.

- [design/execution/](design/execution/README.md) — the dispatch-vs-execute
  split, the deferred-dispatch scheduler, tail-call rewriting, and the per-call
  region lifecycle.
- [design/memory-model.md](design/memory-model.md) — value ownership, region
  lifetime erasure, lifting, and lexical closures.
- [design/per-call-region/](design/per-call-region/README.md) — the
  single-owner contract for the per-call region anchor.
- [design/typing/](design/typing/README.md) — `KType`, dispatch by signature,
  records and tagged unions, plus the module language (`MODULE` / `SIG`,
  ascription, functors, and the roadmapped implicit-search and axiom stages). A
  subdirectory because the type and module systems share one scheduler-driven
  elaborator and nominal-identity carrier.
- [design/functional-programming.md](design/functional-programming.md) — function values, tail calls, signature-driven evaluation.
- [design/expressions-and-parsing.md](design/expressions-and-parsing.md) — the parse pipeline and `KExpression` shape.
- [design/operators.md](design/operators.md) — the `OP` / `GROUP` declaration surface: quoted symbols, chaining modes, the infix combiner, and type-gated shadowing.
- [design/metaprogramming.md](design/metaprogramming.md) — quotation plus splicing: expression values, `EVAL` splicing in place, and the block-level EVAL barrier.
- [design/error-handling.md](design/error-handling.md) — `KError`, propagation, and frame attribution.

[design/effects.md](design/effects.md) captures one further cross-cutting design ahead of
implementation: in-language monadic side effects — a `Monad` signature in Koan with concrete
effect modules (`Random`, `IO`, `Time`) ascribing it. Implementation is tracked in
[roadmap/libraries/monadic-side-effects.md](roadmap/libraries/monadic-side-effects.md).

Future work lives in [roadmap/](roadmap/) — one file per work item, with `Requires:` /
`Unblocks:` cross-links. Its [README](roadmap/README.md) groups work into project
subdirectories — each with its own README naming the project and listing its ready-to-start
items — and derives a "Next items" list, everything with no still-open prerequisite, from
those cross-links (`tools/doclinks.py sync-next`).

The [workgraph/](workgraph/README.md) crate — the scheduler and region-memory
library Koan embeds — carries its own design and roadmap trees, so it reads as a
standalone library rather than as one of Koan's internals. Work items cross-link
across the two trees and `doclinks` gates the pair as one dependency graph, but
each tree derives its own "Next items" list. The boundary between them —
what is library, what is Koan — is
[design/scheduler-library.md](design/scheduler-library.md).
