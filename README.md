# Koan

A functional, graph-based language with a metaprogrammable expression syntax and an ML-like module system.

## Build

Standard Cargo project, edition 2024.

```sh
cargo build                                        # the modules the rewrite keeps
cargo build --features pending_rewrite             # plus the old runtime and the `koan` binary
cargo build --features pending_rewrite --release   # optimized
```

The runtime is being rewritten from the ground up; the old one, and the single
binary target `koan` that drives it, build only under the `pending_rewrite`
feature (see [TEST.md](TEST.md#the-pending-rewrite)).

## Run

The CLI reads source from a file (first argument) or from stdin:

```sh
cargo run --features pending_rewrite -- path/to/program.koan
echo 'PRINT "hello"' | cargo run --features pending_rewrite
```

The builtins wired into the default scope include `LET`, `PRINT`, and the two callable binders `EXPR` (a keyworded, dispatch-reached definition) and `FN` (a lambda); the nominal-type declarators `UNION` and `NEWTYPE`; the control forms `MATCH <value> -> :<Type> WITH (<branches>)`, `TRY (<expr>) -> :<Type> WITH (<branches>)`, and `CATCH`; the module forms `MODULE`, `SIG`, `USING`, the `:!` / `:|` ascription operators, and `TYPE OF <value>` (a value's own type — a module's is its signature); the arithmetic and comparison operators `+ - * / < <= > >=` and `AND`, and the type-union operator `|` building `:(A | B)` (chained runs like `1 < 2 < 3` or `A | B | C` reduce per their operator group's mode — see [expressions and parsing](old_design/expressions-and-parsing.md)); the operator declarators `OP` and `GROUP`, with which a module declares its own chainable operators (see [operators](old_design/operators.md)); `CLOSE OVER (<captures>) (<block>)`, which runs a block over a region of its own so the value it yields copies its captures instead of pinning the frames it was built in (see [lazy closures](old_design/lazy-closures.md)); and the `#` / `$` quote and eval sigils — one file per builtin under [src/builtins/](src/builtins), pulled together by [seed_builtins](src/builtins.rs). See the [tutorial](tutorial/README.md) for a feature-by-feature walkthrough, and [tutorial/reference.md](tutorial/reference.md) for a one-page surface reference.

User-defined functions declare a return type in the `-> Type` slot; the scheduler enforces it at runtime via `KErrorKind::TypeMismatch` when the body produces a value whose type doesn't match. `Any` is the no-op fast-path. The surface-declarable types are `Number`, `Str`, `Bool`, `Null`, `:(LIST OF Elem)`, `:(MAP Key -> Val)`, `:(FN :{arg :Arg} -> Out)` (a lambda type; the parameter list is a record type, so `:{}` is the nullary form), `:(EXPR (<head>) -> Out)` (an expression shape — the type of a keyworded definition, optionally under a `FOR ALL (<names>)` quantifier group), `Type`, `Module`, `Signature`, `KExpression`, and `Any`; nominal types declared with `NEWTYPE`/`UNION` carry their own names. Parameterized type expressions use the glued-right `:` sigil opening an S-expression group; bare types like `Number` and ascriptions like `x :Number` may write the sigil but don't require it on a non-parameterized atom.

Example:

```
LET x = 42
PRINT "hello"
EXPR (ECHO x :Number) -> Number = (x)
LET y = (ECHO 21)
```

Indentation forms blocks (2-space increments, no tabs); `(` `)` group sub-expressions; `'…'` and `"…"` are string literals; numbers, `true`/`false`/`null` are literals. The lexer sorts non-literal atoms into three classes: **keywords** — pure-symbol tokens (`=`, `->`) or alphabetic tokens with ≥2 uppercase letters and no lowercase (`LET`, `THEN`) — are dispatch markers; **type references** are uppercase-leading with at least one lowercase letter (`Number`, `Str`, `KExpression`, `MyType`); everything else (lowercase / snake_case) is an identifier. An uppercase-leading token that fits neither shape (a lone capital, or all-caps-with-digits) is a parse error.

For a walk-through of the language surface with runnable snippets, see the [tutorial](tutorial/README.md).

## Test

```sh
cargo test                               # the kept modules' unit tests
cargo test --features pending_rewrite    # the old runtime's suite as well
cargo test parse::                       # tests under one module
```

Each module keeps its tests in a `#[cfg(test)] mod tests` block alongside the code (parser, scheduler, dispatch, and interpreter all have suites). For the full testing and linting workflow — including the Miri audit slate that signs off the memory model under tree borrows — see [TEST.md](TEST.md).

Measurement scaffolding that no build ships — the counting global allocator, the debug reach-tightness report, and the recorded koan programs the allocation baselines are read over — lives outside `src/` under [audit/](audit/README.md), which carries the charter for the split plus the committed baseline table and the script that reproduces it.

## Architecture

The pipeline is three stages, split across two top-level modules:

```
source ──▶ parse ──▶ dispatch ──▶ execute
        KExpression  DispatchOutcome  KObject
```

`memory`, `parse`, `builtins`, and `machine` are sibling crate-top modules; `machine` owns dispatch and execute. [src/main.rs](src/main.rs) reads the source and hands it to `interpret_with_writer_path`, which stands up the scope pair, seeds the builtins, and drains the scheduler.

### parse — text → `KExpression` tree, and the vocabulary it produces

Entry point: `parse` in [src/parse.rs](src/parse.rs). It runs in two phases, splitting layout from vocabulary:

1. [sexlex](sexlex/README.md) — the workspace crate that reads the text into a layout tree of atoms, strings, commas and groups. Whitespace separates, three bracket families group, quotes delimit strings, indentation nests lines, and adjacency between siblings is recorded rather than interpreted. It knows no koan.
2. [lower.rs](src/parse/lower.rs) — walk that tree into `KExpression`s. This is where koan's vocabulary enters: sigils (`#`, `$`, `:`) and the groups they take, the redundant-wrapper peel, brace pairing, collection adjacency, and spans.

Two files serve the lowering:

- [atom.rs](src/parse/atom.rs) — classify one atom: split it on its colons (`x:Number` is the word `x` and the type `Number`) and tag each piece as a literal, keyword (pure-symbol like `=`, `->`, `:|`, or alphabetic with ≥2 uppercase letters and no lowercase — `LET`, `THEN`), type name (uppercase-leading with at least one lowercase — `Number`, `KFunction`, `Ordered`), identifier, or compound (member access, suffix operators).
- [operators.rs](src/parse/operators.rs) — table of compound-atom operators (`.`, `?`); add a row to extend.

The output is one [`KExpression`](src/parse/ast.rs) per top-level line: an ordered sequence of `ExpressionPart`s (`Keyword`, `Identifier`, `Type`, nested `Expression`, `ListLiteral`, or typed `Literal`). The `Keyword` vs slot split is the parser's contract with dispatch: only `Keyword` parts contribute fixed tokens to a signature's bucket key; `Identifier`, `Type`, literals, and sub-expressions all become slots that compete on type specificity.

`parse` owns what it produces, not just the walk that produces it: [labels.rs](src/parse/labels.rs) mints and interns every symbol, [ast.rs](src/parse/ast.rs) defines the syntax types and the [`NodeCache`](src/parse/ast/shape.rs) each node fills at construction, and [forms.rs](src/parse/forms.rs) holds `FORMS` — the one table spelling every builtin form's bucket key, tagged by a `FormId`, carrying the binder facts, the lazy slots and the reserved bit each form's readers ask for. A node probes that table once; the close-inference rules and the miss diagnostics name a form by its tag rather than respelling its key.

`KExpression` is a `Copy` handle: its parts run and every string in it borrow the program storage the parse bumped them into. The scheduler dispatches a separate [`WorkingExpression`](src/machine/model/ast/working.rs), which is where a resolved sub-result gets spliced back in — so an expression *value* can never carry one. A node only reaches the value channel wrapped in the [program-storage marker](src/parse/ast/program.rs), which types the tier the channel's verdicts assume. See [src/parse/README.md](src/parse/README.md).

### dispatch — `KExpression` → `DispatchOutcome` against a `Scope`

A [`Scope`](src/machine/core/scope.rs) is a lexical environment: parent link, name → value bindings, an indexed list of functions, and a pluggable output sink. [`resolve_dispatch`](src/machine/execute/decide/resolve_dispatch.rs) walks the scope chain in a single pass and returns a [`DispatchOutcome`](src/machine/execute/decide/resolve_dispatch.rs) — `Resolved` (a unique pick, plus the bare-name slots to auto-wrap), `Ambiguous(n)` (strict-mode tie), `ParkOnProducers` (wait on a still-finalizing earlier binder), `UnboundName`, or `Unmatched` (a real dispatch failure, carrying a forgotten-quote hint when one applies). Every eager-shaped child is submitted before this runs — which slots stay raw is the `lazy_slots` stamp of the node's cached [`FORMS`](src/parse/forms.rs) entry — so dispatch selects over landed values. [`ExpressionSignature`](src/machine/model/types/signature.rs)s mix fixed `Token`s and typed `Argument` slots; on `Resolved` the resolved function binds its arguments, ready to run but not yet executed.

Runtime values are [`KObject`](src/machine/model/values/kobject.rs) (scalars, collections, expressions, function references); the cross-cutting `Parseable` trait lives in [ktraits.rs](src/machine/model/types/ktraits.rs). Builtins are registered in [builtins.rs](src/builtins.rs) and produce the default root scope.

Errors are first-class via [`KError`](src/machine/core/kerror.rs) — a `Done(Err(KError))` outcome propagates structured failures (type mismatches, unbound names, dispatch failures, shape errors) along the scheduler's dependency edges, accumulating call-stack frames as it walks. `TRY (<expr>) WITH (<branches>)` catches in-language; uncaught errors short-circuit to the top level and the CLI formats them with frames. See [old_design/error-handling.md](old_design/error-handling.md) for the per-arm `it` shape and the privilege boundary that keeps builtin and user errors disjoint.

### execute — run the DAG

The [`Scheduler`](workgraph/src/scheduler.rs) — the [workgraph](workgraph/README.md) crate's — holds a slot table of in-flight work plus a push/notify dependency graph over first-class edges, and its `drain` owns the pop loop; [`KoanRuntime`](src/machine/execute/harness.rs) owns the scheduler beside the koan-side `Host` whose `step` is the drain callback. Callers submit a top-level block via the harness's `enter_block`; each slot's decide spawns sub-Dispatches for the expression's nested parts and parks the parent as a dep-finish until its deps terminalize. When a producer finalizes, a single walk delivers its terminal into every waiting edge's destination region and wakes any consumer whose pending count hits zero — no polling, no result-table sweep, and the producer's slot reclaims behind the walk. Tail returns (an `Action::Tail` lowered to `Outcome::Continue`) rewrite the slot's own work in place rather than allocating a new slot. See [the execution model](old_design/execution/README.md).

[`interpret`](src/machine/execute/interpret.rs) is the glue: parse the source, allocate the run-root scope and its `RunScope` child (`unseeded_scopes`), establish the run frame, seed the builtins against that frame's type registry (`seed_builtins`), hand the top-level block to `enter_block`, drain the scheduler, then `read_result` each top-level node. `PRINT` output flows through the scope's pluggable writer (default stdout; tests swap in a shared `Vec<u8>` buffer to read it back), and every value the program allocated dies with the per-run `KoanRegion` when `interpret` returns.

## Source layout

The crate splits into five top-level modules: [memory/](src/memory) (where a
value lives and how long), [parse](src/parse.rs) (text → `KExpression`, plus the
symbol, AST and form-table vocabulary that output is written in),
[builtins/](src/builtins) (the K-language standard library, one file per
builtin), [type_lattice/](src/type_lattice.rs) (the closed algebra over interned
type nodes — see [src/type_lattice/README.md](src/type_lattice/README.md)),
and [machine/](src/machine) (the execution engine that consumes a
`KExpression`). `parse` splits into [ast/](src/parse/ast.rs) (the syntax types,
the node cache and the eternal-tier program marker),
[labels.rs](src/parse/labels.rs) (`Symbol`, the content-digest handle every
syntactic label travels as, beside its interner) and
[forms/](src/parse/forms.rs) (`FORMS` and the binder / lazy-slot / slot-layout
facts riding its entries). `machine` further
splits into [model/](src/machine/model) (the value/type vocabulary —
[ast.rs](src/machine/model/ast.rs) for what the machine *does* with a parsed node,
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
`region.rs` (allocation),
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
[registries.rs](src/machine/model/registries.rs) (`RunRegistries`, the run frame's
owned bundle of that registry beside the label interner — see
[src/parse/README.md](src/parse/README.md) § Labels),
[builtins.rs](src/builtins.rs) (registry),
[constructors.rs](src/machine/execute/decide/constructors.rs) (shared structure),
[typed_field_list.rs](src/machine/model/types/typed_field_list.rs) (helper).

`type_lattice/` reuses those names — `KType`, `TypeNode`, `TypeRegistry` — under
its own path. The machine still reaches types through
[types/](src/machine/model/types); pointing every caller at the lattice and
deleting the files above is
[integrate the type lattice](roadmap/old_refactor/type-lattice-integration.md).

```
src/
├── main.rs              CLI entry point — reads source, calls interpret_with_writer_path
├── lib.rs               library facade — declares `memory`, `parse`, `builtins`, and `machine` so integration tests under tests/ link against the same module graph, and re-exports workgraph's DAG scheduler as `koan::scheduler`
├── tests.rs             `#[cfg(test)]` crate-wide test scaffolding — installs audit/'s counting global allocator for the lib-test binary and exposes the tally fixed-cost measurements read
├── source.rs            source-span and provenance carrier for errors
├── memory.rs            pub mod memory — where a value lives and how long: Koan's instantiation of workgraph's region substrate, and every substrate name Koan spells
├── memory/
│   ├── substrate.rs        the crate's only import of workgraph::witnessed / hashbrown / allocator_api2 — one Koan-bound alias per library generic (Delivered / Sealed / Opened / Witnessed / Retained / RegionHandle / FoldedPlacement / Sectioned / StepContext …), plus a verbatim re-export of the names that take no Koan parameter
│   ├── region.rs           KoanStorageProfile, KoanRegion (= Region<KoanStorageProfile>), FrameStorage (the per-call region owner), the RegionBrand / FoldingBrand / SubstrateDoor allocation veneer with the residence derivations off a brand (region_owner / frame / parent_frame_pin), run_root_storage and the bump-backed table constructor
│   ├── frame.rs            Frame<F> — the per-call region shell (envelope + storage), generic over the resident family it seats, with the two doors (open_under / adopting) that are the only way a storage and a resident get paired — plus FrameReach / FrameCoverage, the reach-evidence aliases
│   ├── slots.rs            SlotState / SlotArray — the layout-addressed counterpart of the bump-backed map: a fixed run of Empty | Claimed(P) | Bound(V) cells in one bump allocation, beside the live-claim counter that makes "nothing in flight" an O(1) read
│   ├── scope_id.rs         ScopeId — counter-minted, position-independent scope identity for per-declaration types; an identity source, never looked up against
│   └── program.rs          ProgramStorage / ProgramBrand — the eternal-tier region program text and its parsed AST are bumped into, above the run root
├── parse.rs             pub mod parse — the parser and what it produces: the label vocabulary, the syntax AST, and the form table
├── parse/
│   ├── lower.rs            layout tree → KExpressions: sigils, the redundant-wrapper peel, adjacency, spans
│   ├── atom.rs             classify one atom — the colon split, compound-operator desugaring
│   ├── brace.rs            DictFrame state machine for `{k: v}` / `{x = 1}` pairing
│   ├── operators.rs        operator registry
│   ├── labels.rs           Symbol — a label's 128-bit content digest — plus LabelInterner (the run's digest→text side table, read only when rendering), the four classified symbol wrappers, BindKind, the token classifiers and the identity hasher every symbol-keyed table uses
│   ├── ast.rs              the syntax AST: KLiteral / ExpressionPart / KExpression — Copy handles over bumped slices, with a Type part carrying only its TypeSymbol
│   ├── ast/
│   │   ├── shape.rs        PartClass / DispatchShape / KeyElement / UntypedKey + NodeCache, the one structural cache both node families carry (stored key, shape, operator probe, FORMS entry, binder plan) and the readers that fill it
│   │   └── program.rs      ProgramExpression / ProgramNode — the eternal-tier marker that makes "this node's parts run is hosted in program storage" a type
│   ├── forms.rs            FORMS — every builtin form's full bucket key spelled once, tagged by FormId, carrying its binder facts, its lazy slots and its reserved bit; KEYWORDS, the one key matcher, and the single table probe
│   └── forms/
│       ├── binder.rs       BinderFacts and the structural extractors: which name and bucket key(s) a binder form declares, read off the node's cached entry
│       ├── lazy.rs         LazyKinds — which part kinds a slot captures raw instead of evaluating
│       └── layout.rs       SlotLayout — a body's value binders as a symbol-sorted run, computed where the shape is lexically fixed
├── builtins.rs          register_builtin, unseeded_scopes(), seed_builtins()
├── builtins/            one file per builtin (body + register paired)
│   ├── let_binding.rs
│   ├── print.rs
│   ├── attr.rs
│   ├── fn_def.rs             EXPR / FN — the two callable definition surfaces
│   ├── fn_def/signature.rs      head / record-schema parsing (keywords, slots, `_` wildcards)
│   ├── fn_def/quantifiers.rs    the `FOR ALL (<names>)` group a quantified head binds
│   ├── fn_def/return_type.rs    return-type slot elaboration
│   ├── fn_def/param_refs.rs     parameter-reference resolution
│   ├── fn_def/finalize.rs       seal the function once its slots resolve
│   ├── match_case.rs         MATCH — branch by union member (OVER) or by the scrutinee's runtime type
│   ├── try_with.rs           TRY (<expr>) WITH (<branches>) — catch runtime errors
│   ├── catch.rs              CATCH — error-handling primitive
│   ├── branch_walk.rs        the shared member-arm parser + MATCH's by-member and by-type walkers + TRY's member walker + shared arm-tail machinery
│   ├── result.rs             Result — the prelude two-member union (Ok / Error)
│   ├── error_union.rs        KError — the prelude union of every catchable error kind
│   ├── parameterized_types.rs  keyworded type-language overloads (LIST OF / MAP _ -> _ / the `FN :{…} -> R` lambda type)
│   ├── type_ops.rs           WITH — infix signature specialization; TYPE OF — value → type
│   ├── type_ops/with.rs               WITH — abstract-slot pinning + manifest fixity
│   ├── type_ops/type_of.rs            TYPE OF — a value's own type (a module's is its signature)
│   ├── union.rs              UNION — sum-type declaration (dissolves to one newtype per variant, joined by an anonymous union)
│   ├── type_union.rs         `|` — the `:(A | B)` anonymous-union type constructor
│   ├── record_projection.rs  FROM — `(x y) FROM r` re-tags a record value's carried type to the named fields
│   ├── nominal_schema.rs     shared Action-harness field-list elaboration for UNION / NEWTYPE record repr
│   ├── newtype_def.rs        NEWTYPE — scalar repr, the `:{…}` record repr, and the `(Param… AS Name)` constructor-family mint
│   ├── module_def.rs         MODULE — the body's child scope and the declaration window it announces into, which co-declares a mutually-recursive nominal group (the announcement scan itself is model/binder.rs)
│   ├── op_def.rs             OP / UNARY OP — declare a chainable operator over an operand type
│   ├── group_def.rs          GROUP — a module bundling mutually chainable operators under one reduction mode
│   ├── sig_def.rs            SIG
│   ├── val_decl.rs           VAL (SIG-body value-slot declarator)
│   ├── type_decl.rs          TYPE — SIG-body abstract type-member declarators (bare + higher-kinded)
│   ├── ascribe.rs            :| / :! module ascription
│   ├── using_scope.rs        USING — lexical-scope introduction
│   ├── test_support.rs
│   └── eval.rs               # surface form `$(expr)`
├── type_lattice.rs   pub mod type_lattice — the closed algebra over interned type nodes: the vocabulary, the registry, the identity recipe, the relations and the unifier, over labels, `ScopeId` and the region bump seam and nothing else
├── type_lattice/
│   ├── node.rs           TypeNode — one interned type's content; every child position is a KType handle, so a node is shallow
│   ├── handle.rs         KType — the Copy content-digest handle, the pinned builtin constants, and the name/kind readings off one
│   ├── digest.rs         TypeDigest and the one identity recipe: the hand-written tag table, one layer deep, plus the schema and component digests
│   ├── registry.rs       TypeRegistry — the region-hosted interning table (each node beside its probe flags) and the heap verdict cache, the composite doors, canonical `union_of`, the canonicalizing `shape_type` and `signature`
│   ├── kind.rs           KKind — the shallow kind a type-accepting slot admits
│   ├── record.rs         Record — a Copy view over a region slice of BinderSymbol-keyed fields, backing record types and lambda parameter identity
│   ├── shape.rs          DispatchTokenElement / DeferredReturnSurface / Specificity — the non-type payloads a node carries
│   ├── operators.rs      ReductionMode / FoldDirection — how a run of a signature's operators reduces, which is part of the signature's identity
│   ├── schema.rs         SigSchema over symbol-sorted Members tables, the SchemaDraft the signature door canonicalizes, the channels' canonical orders, and the shape readers
│   ├── walk.rs           Variance and the two drivers every structural recursion goes through
│   ├── walk/unary.rs     the arm table behind `visit` and `rebuild`, with the descent knobs and the position context
│   ├── walk/binary.rs    the pairing table behind `lockstep`: width verdicts, the variance flip, and the rebuild door
│   ├── order.rs          is_subtype_of — the one order, as a single Lockstep instance — plus is_more_specific_than and satisfied_by
│   ├── lattice.rs        join (subsumption-or-union, not a walk) and meet (the rebuilding Lockstep instance)
│   ├── unify.rs          admits_with and the Collector: contributions solved by maximum, minimum, or the declared bound
│   ├── substitute.rs     the quantifier and member substitutions, and the three slot_* relations that are each one of them composed with an ordinary relation
│   ├── sig_relations.rs  sig_subtype and its failure record, keyworded selection, meet_schemas, and shape_specificity
│   ├── window.rs         RecursiveGroupWindow and seal_group — the open/seal doors and the Tarjan component pass behind them
│   └── render.rs         surface-syntax rendering — the one recursion written by hand, over the registry and the label interner
├── machine.rs           pub mod core / model / execute
└── machine/
    ├── model.rs            re-exports from model::types and model::values
    ├── model/
    │   ├── ast.rs                 what the machine does with a parsed node: literal lowering, part resolution to a cell, impl Parseable — inherent impls on parse's syntax types
    │   ├── ast/
    │   │   ├── shape.rs           Part / FieldSlot / PartSummary — the field-list part view both expression families answer in
    │   │   └── working.rs         WorkingExpression / WorkingPart — the scheduler's own node, the only one that can hold a spliced sub-result
    │   ├── binder.rs              what a binder *does* with what parse read: the scope install, the refusal rendering, MACHINE_BINDERS and the module-body announcement scan
    │   ├── close_inference.rs     CLOSE_RULES — per-FormId capture rules for the implicit-close walk, probed off the node's cached form
    │   ├── miss_diagnostics.rs    MISS_DIAGNOSTICS — per-FormId renderers for a keyword spine that dispatched to nothing, plus the reserved-key check the overload write door reads
    │   ├── pair_list.rs           `<name> <slot>` pair lists over a built parts run: parse_pair_list (classified names + slots) and parse_type_tag_names (the variant-tag pre-scan)
    │   ├── operators.rs           OperatorGroup registry record — chainable-operator precedence/associativity
    │   ├── registries.rs          RunRegistries — the run frame's owned bundle of run-lifetime lookup state (the TypeRegistry beside the LabelInterner)
    │   ├── types.rs
    │   ├── types/
    │   │   ├── ktype.rs           KType — the Copy content-digest handle for slots, return types, and runtime values
    │   │   ├── kkind.rs           KKind — the shallow dispatch *kind* of a type (the OfKind expectation)
    │   │   ├── node.rs            TypeNode — one interned type's content, the thing a KType handle names
    │   │   ├── registry.rs        TypeRegistry — the run-frame-owned interning graph and verdict cache
    │   │   ├── record.rs          Record<V> — ordered BinderSymbol-keyed map over a Vec<(BinderSymbol, V)>, identity on the key's symbol bits, backing record-type schemas and lambda parameter identity
    │   │   ├── ktype_predicates.rs   dispatch-time predicates (matches_value, accepts_part, is_more_specific_than)
    │   │   ├── ktype_resolution.rs   builtin type-name elaboration (from_symbol, twelve symbol compares against builtin_names)
    │   │   ├── builtin_names.rs   the twelve builtin type names as StaticName<TypeSymbol>s, each beside the KType it lowers to
    │   │   ├── resolver.rs        Elaborator + elaborate_type_expr — scheduler-aware type-name elaboration with placeholder parking (no cache tier; interning already makes a re-elaborated form yield the same handle)
    │   │   ├── recursive_group_window.rs   RecursiveGroupWindow — the pre-seal group window and the SCC seal that interns its members
    │   │   ├── sig_schema.rs      SigSchema + sig_subtype — a signature type's owned schema and the subtyping relation
    │   │   ├── signature.rs       ExpressionSignature, Specificity — dispatch shape + tie-breaker
    │   │   ├── ktraits.rs         Parseable / Serializable
    │   │   └── typed_field_list.rs  shared parser for `(name :Type ...)` schemas
    │   ├── values.rs
    │   └── values/
    │       ├── kobject.rs         runtime value type
    │       ├── container_substrate.rs  ContainerSubstrate<'a, C> — the index-generic region-resident substrate (sectioned cells + run union + copy cost), Copy and bump-hosted in every arm; C is RecordLayout (a symbol-sorted &[Symbol] slice), a dict's frozen &BumpBackedMap, or a list/payload marker
    │       ├── cell.rs             Held / Carried — the owned and borrowed value cells, with the carrier aliases each travels in (CarriedFamily, DeliveredCarried, SplicedCell)
    │       ├── kkey.rs            KKey — hashable scalar wrapper for dict keys
    │       ├── named_pairs.rs     shared (name, value) ordered-list helper
    │       ├── module.rs          Module — first-class module values, their sealed self-sig content, and the ModuleRefFamily a region-stored &Module erases through
    │       └── coerce.rs          coerce_object_into — the ascription-barrier walk that rebuilds a value under a different binding of a signature's abstract members (an opaque view's members are born coerced)
    ├── core.rs            module surface for core/
    ├── core/
    │   ├── bindings.rs    Bindings façade — two RefCells: the value channel (keyed or slotted, owning its own claims) and the keyed channel beside it (types/functions/operators + the claim store); the firm write_value / write_type / write_operator_group primitives and the visibility-aware lookup_value/lookup_type/lookup_function_stored surface (raw map accessors are #[cfg(test)]); no verb holds both cells, and nothing else is interior-mutable
    │   ├── bindings/
    │   │   ├── values.rs  ValueStore / ValueAddress / DataEntry — the value channel in both representations (a bump-backed map of three-state cells, or a SlotArray sized by the body's SlotLayout), owning this channel's claims so a value name's whole state is one cell read
│   │   ├── claims.rs  Claim / ClaimStore — the in-flight binder claims of the channels that have no cell of their own (by_type / by_bucket read paths, by_statement retirement run), sized at the block fan-out
    │   │   ├── ops.rs     WriteOp / TypeWritePolicy — a binding-table write as outcome data, and the single apply interpreter the run loop drives
    │   │   └── gate.rs    WriteGate — the zero-sized capability every table write verb requires, minted only inside crate::machine (run loop + unpublished-scope construction door)
    │   ├── kerror.rs      KError, KErrorKind, TraceFrame — structured runtime errors, with the caught record's field labels as one StaticName<ValueSymbol> group
    │   ├── scope.rs       Scope — lexical environment: the bump-resident struct, the ScopeRefFamily / RegionScopeFamily reattach families a region-stored &Scope erases through, CallFrame (= Frame<ScopeRefFamily>) and its scope_id read, the frame doors (open_frame / adopt_as_run_frame, thin callers of memory's Frame::open_under / Frame::adopting), its allocators (alloc_run_root / alloc_child_under / … , bumped at 'a; alloc_child_transparent through the crossing born door) with their private constructors, and small accessors (children below)
    │   ├── scope/
    │   │   ├── resolve.rs     name-resolution ladders — value / type / operator-group lookup, walk_chain / resolve_builtin_first, visibility cutoff, builtin-shadow consults
    │   │   ├── registry.rs    write doors — the seal_* construction halves of the value binds, the submission-channel placeholder installs, the owns-its-bindings write-target guard, and the *_direct writes for unpublished scopes
    │   │   ├── reach.rs       reach / carrier derivation — resident value / type carriers, envelope sealing, copy-free / copying adoption, and the module store folds
    │   │   └── copy.rs        the environment copy behind the Consolidate verb — rebuilds a callable's per-call captured chain at the destination region, memoized per source scope so cycles terminate and siblings share
    │   ├── seals.rs       OverloadSeal / GroupSeal — the registration bundles a dispatch or operator write takes, computed at seal time so no write verb opens a carrier
    │   ├── statement_id.rs  StatementId — counter-minted, never-recycled identity of one submitted statement; what a binding entry's Installer names, so declaration identity borrows nothing from the scheduler
    │   ├── lexical_frame.rs  LexicalFrame — immutable cactus-chain (scope_id, index, parent) attached to every dispatched node
    │   ├── kfunction.rs   KFunction, Body — body shapes plus the dispatch-to-execute bridge
    │   └── kfunction/
    │       ├── body.rs              Body / ReturnContract
    │       ├── exec.rs              run_user_fn — innermost body executor; returns a scheduler-unaware ExecOutcome
    │       ├── action.rs            Action — the scheduler-aware currency a builtin returns: the WriteOp effects it decided plus its ActionKind continuation (types only)
    │       ├── block_tail.rs        the "run a block, return the tail" constructors — the sole Action::Tail sites: block_tail (EVAL / MATCH / TRY arms / USING) and fresh_cart_tail (CLOSE OVER), sharing one statement-split freeze
    │       └── pick.rs              per-bucket tournament selecting the most-specific overload
    ├── execute.rs
    └── execute/
        ├── harness.rs     KoanRuntime (Scheduler + Host side by side) — Host::step is the drain callback (open the sealed continuation at one rank-2 step brand, decide, drain binding writes), Host::apply is the sole &mut Scheduler code (wire_deps, the Outcome → StepVerdict map), plus run_action (lowers a builtin Action to an Outcome, pure), run_program, and the AST-aware submission wrappers (enter_block / dispatch_in_scope / dispatch_in_own_scope / dispatch_body / submit_dep_finish_witnessed_in_own_scope)
        ├── interpret.rs   the embedder API: the interpret → interpret_with_writer → interpret_with_writer_path ladder
        ├── nodes.rs       node types: the NodeWork re-export from the scheduler, plus SlotFrame / NodeScope / NodePayload / ChainOp
        ├── producer_id.rs  ProducerId — the opaque park token everything below the drive loop stores, compares, and hands back but cannot open (both conversions pub(in crate::machine::execute)), plus deps_on / extend_deps_on, the single verb pair that spends one
        ├── outcome.rs     Outcome — the unified scheduler-step currency (Done / Continue / Park / Forward) + Replacement (a Continue's work coupled to its frame placement; constructor-minted host brand) + Continuation (Ready / Catch) + NodeContinuation (the slot's obligation as data beside a two-tier ContinuationCall: Bumped &dyn Fn / Boxed Box<dyn FnOnce>) + the Await envelope builder (sole finish-carrying-Park constructor) + the erase doors (erase_bumped / erase_boxed) and the generic adapters composed before them (gated / sealed_done / catching / decide_only); AST-free (carries DepRequest as an opaque type)
        ├── ambient.rs     AmbientContext — the per-step ambient state (active frame, run frame, slot payload, declared-return obligation)
        ├── run_frame.rs   RunFrame — the run's own frame: the CallFrame adopting the run-root scope beside the RunRegistries every step consults and the RunWriter PRINT writes to, all owned outright and dropped at run teardown
        ├── step.rs        StepCarried / StepAllocator — the step-brand layer: the Done-arm carrier confined to the step that built it, the construction context over that step's destination frame, and the two RegionBrand doors that mint a step-branded product
        ├── decide.rs      classify_dispatch (the decide) + decide_tail + classify_dispatch_shape; submit/ (binder-aware submit_expression chokepoint), literal/ (aggregate-literal lowering), ctx/ (DecideCtx — the scheduler-free step context), resolve/ (Resolution — THE bare-name ladder), exec/ (decide-side invoke), keyworded/, fn_value/, single_poll/, head_deferred/, apply_callable/, operator_chain/, field_list/, constructors/, resolve_dispatch/, resolve_type_identifier/ submodules
        └── lift.rs        lift_kobject — rebuild values across per-call region boundaries
```

## Design and roadmap

A module's design doc is the `README.md` in its own source directory, linked
from that module's top-of-file comment. The kept modules carry theirs:

- [src/parse/README.md](src/parse/README.md) — the division of labour with
  `sexlex`, the label vocabulary and its content-digest identity, the borrowed
  splice-free AST and its structural cache, and the `FORMS` table every node is
  classified against at construction.
- [src/memory/README.md](src/memory/README.md) — the three storage tiers, the
  frame shell that names no Koan value, the one-place substrate alias layer, the
  two table shapes, and the drop-freeness the region discipline rests on.
- [src/type_lattice/README.md](src/type_lattice/README.md) — the closed algebra:
  digest identity, the node vocabulary, the interning registry, the one order
  and the lattice operations over it, and the unifier that solves a quantified
  position.
- [sexlex/README.md](sexlex/README.md) — the layout half of the parser: what it
  decides, the three things it refuses, and the three indentation regimes.
- [cellgraph/README.md](cellgraph/README.md) — the cell substrate's contract and
  verbs, with [cellgraph/src/graph/README.md](cellgraph/src/graph/README.md) for
  matrix liveness, the sealed tier and pricing, and
  [cellgraph/src/tree/README.md](cellgraph/src/tree/README.md) for tree cells.

Each rewrite item writes its own module's README the same way, fresh against the
code it lands. The topical tree the old runtime was documented under is frozen at
[old_design/](old_design/README.md): it is requirements reading for the runtime
behind `pending_rewrite` — never added to, never edited, and carrying no link
into a kept module.

Future work lives in [roadmap/](roadmap/) — one file per work item, with `Requires:` /
`Unblocks:` cross-links. Its [README](roadmap/README.md) groups work into project
subdirectories — each with its own README naming the project and listing its ready-to-start
items — and derives a "Next items" list, everything with no still-open prerequisite, from
those cross-links (`tools/doclinks.py sync-next`).

The [cellgraph/](cellgraph/README.md) cell substrate carries its design in its
own module READMEs and its open work in a roadmap tree of its own, so it reads as
a standalone library rather than as Koan's internals; work items cross-link
across the trees and `doclinks` gates them as one dependency graph, but each tree
derives its own "Next items" list. The
[workgraph/](workgraph/README.md) scheduler is the old runtime's and is replaced
by a fresh crate ([roadmap/rewrite/scheduler-on-cellgraph.md](roadmap/rewrite/scheduler-on-cellgraph.md));
its design and roadmap trees are retired under `old_` prefixes, as is the
boundary doc [old_design/scheduler-library.md](old_design/scheduler-library.md).

[sexlex/](sexlex/README.md) is the third crate Koan embeds: the layout half of
the parser, with no vocabulary of its own. Unlike the other two it carries no
roadmap tree — its README is the whole design statement, and the crate doc on
[sexlex/src/lib.rs](sexlex/src/lib.rs) is the precise form of the five rules
about whitespace, brackets, quotes, adjacency and indentation it rests on.
