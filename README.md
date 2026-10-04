# Koan

A functional, graph-based language with a metaprogrammable expression syntax and an ML-like module system.

## Build

Standard Cargo project, edition 2024.

```sh
cargo build                                        # the library and the `koan` binary
cargo build --release                              # optimized
```

The single binary target `koan` does not yet run `MATCH`, `TRY`, `CATCH`,
`Result` or the module expression shapes
([what dispatch runs](src/dispatch/README.md#what-dispatch-runs)).

## Run

The CLI reads source from a file (first argument) or from stdin:

```sh
cargo run -- path/to/program.koan
echo 'PRINT "hello"' | cargo run
```

`PRINT` writes to standard output. A refused load, or an error the program does
not catch, writes `error: <message>` to standard error and exits non-zero.

The surface includes `LET`, `PRINT`, and the two callable binders `EXPR` (a keyworded, dispatch-reached definition) and `FN` (a lambda); the nominal-type declarators `UNION` and `NEWTYPE`; the control forms `MATCH <value> -> :<Type> WITH #{<branches>}`, `TRY (<expr>) -> :<Type> WITH #{<branches>}`, and `CATCH`; the module forms `MODULE`, `SIG`, `USING`, the `:!` / `:|` ascription operators, and `TYPE OF <value>` (a value's own type — a module's is its signature); the arithmetic and comparison operators `+ - * / < <= > >=` and `AND`, and the type-union operator `|` building `:(A | B)` (chained runs like `1 < 2 < 3` or `A | B | C` reduce per their operator group's mode — see [operator groups](src/scope/README.md#operator-groups)); the operator declarators `OP` and `GROUP`, with which a module declares its own chainable operators; and the `#` / `$` quote and eval sigils. See the [tutorial](tutorial/README.md) for a feature-by-feature walkthrough, and [tutorial/reference.md](tutorial/reference.md) for a one-page surface reference.

User-defined functions declare a return type in the `-> Type` slot; a body whose value does not fit it ends in an error value. `Any` is the no-op fast-path. The surface-declarable types are `Number`, `Str`, `Bool`, `Null`, `:(LIST OF Elem)`, `:(MAP Key -> Val)`, `:(FN :{arg :Arg} -> Out)` (a lambda type; the parameter list is a record type, so `:{}` is the nullary form, and a `FOR ALL #[<names>]` group before the parameters makes it quantified, which only a signature's `VAL` member may be), `:(EXPR #(<head>) -> Out)` (an expression shape — the keyword/slot run a keyworded definition registers for dispatch, optionally under a `FOR ALL #[<names>]` quantifier group inside a signature), `Value`, `Type`, `Code` and the kinds of code under it (`Block`, `Expression`, `Declaration`, `Binder`, `Literal`, `Symbol`, `Name`, `Keyword`), `Module`, `Signature`, and `Any`; nominal types declared with `NEWTYPE`/`UNION` carry their own names. Parameterized type expressions use the glued-right `:` sigil opening an S-expression group; bare types like `Number` and ascriptions like `x :Number` may write the sigil but don't require it on a non-parameterized atom.

Example:

```
LET x = 42
PRINT "hello"
EXPR #(ECHO x :Number) -> Number = #(x)
LET y = (ECHO 21)
```

Indentation forms blocks (2-space increments, no tabs); `(` `)` group sub-expressions; `'…'` and `"…"` are string literals; numbers, `true`/`false`/`null` are literals. The lexer sorts non-literal atoms into three classes: **keywords** — pure-symbol tokens (`=`, `->`) or alphabetic tokens with ≥2 uppercase letters and no lowercase (`LET`, `THEN`) — are dispatch markers; **type references** are uppercase-leading with at least one lowercase letter (`Number`, `Str`, `Expression`, `MyType`); everything else (lowercase / snake_case) is an identifier. An uppercase-leading token that fits neither shape (a lone capital, or all-caps-with-digits) is a parse error.

For a walk-through of the language surface with runnable snippets, see the [tutorial](tutorial/README.md).

## Test

```sh
cargo test                               # every module's unit tests
cargo test parse::                       # tests under one module
cargo test -p lattice                    # the symbols, the type lattice and the bump tier
```

Each module keeps its tests in a `#[cfg(test)] mod tests` block alongside the code. For the full testing and linting workflow — including the Miri audit slate that signs off the memory model under tree borrows — see [TEST.md](TEST.md).

Measurement scaffolding that no build ships — the counting global allocator the unit tests bracket allocations with — lives outside `src/` under [audit/](audit/README.md), which carries the charter for the split and the `dhat` profiling workflow.

## Architecture

A program goes through three stages:

```
source ──▶ parse ──▶ load ──▶ run
        KExpression  BodyShape  Value
```

The [binary](src/main.rs) hands the source to [program](src/program/README.md),
which parses it, loads it and runs it under [dispatch](src/dispatch/README.md)'s
`Koan`.

### parse — text → `KExpression` tree, and the vocabulary it produces

Entry point: `parse` in [src/parse.rs](src/parse.rs). It runs in two phases, splitting layout from vocabulary:

1. [sexlex](sexlex/README.md) — the workspace crate that reads the text into a layout tree of atoms, strings, commas and groups. Whitespace separates, three bracket families group, quotes delimit strings, indentation nests lines, and adjacency between siblings is recorded rather than interpreted. It knows no koan.
2. [lower.rs](src/parse/lower.rs) — walk that tree into `KExpression`s. This is where koan's vocabulary enters: sigils (`#`, `:`, and the `$` and `\` marks) and the groups they take — `#` glued to a list or dict literal quotes each element, a mark glued to a paren wraps one keyworded use — the layout-line peel (a written paren is always kept), brace pairing, collection adjacency, and spans.

Two files serve the lowering:

- [atom.rs](src/parse/atom.rs) — classify one atom: split it on its colons (`x:Number` is the word `x` and the type `Number`) and tag each piece as a literal, keyword (pure-symbol like `=`, `->`, `:|`, or alphabetic with ≥2 uppercase letters and no lowercase — `LET`, `THEN`), type name (uppercase-leading with at least one lowercase — `Number`, `KFunction`, `Ordered`), identifier, or compound (member access, suffix operators).
- [operators.rs](src/parse/operators.rs) — table of compound-atom operators (`.`, `?`); add a row to extend.

The output is one [`KExpression`](src/parse/ast.rs) per top-level line: an ordered sequence of `ExpressionPart`s (`Keyword`, `Identifier`, `Type`, nested `Expression`, `ListLiteral`, or typed `Literal`). The `Keyword` vs slot split is the parser's contract with dispatch: only `Keyword` parts contribute fixed tokens to a signature's bucket key; `Identifier`, `Type`, literals, and sub-expressions all become slots that compete on type specificity. Each node also stores how deep its syntax nests ([depth.rs](src/parse/depth.rs)), and a program nested past the one syntax depth limit is refused at parse ([§ The syntax depth limit](src/parse/README.md#the-syntax-depth-limit)).

`parse` owns what it produces, not just the walk that produces it: [ast.rs](src/parse/ast.rs) defines the syntax types and the [`NodeCache`](src/parse/ast/shape.rs) each node fills at construction, and [builtin_shapes.rs](src/parse/builtin_shapes.rs) holds `BUILTIN_SHAPES` — the one table spelling every builtin bucket as a typed run, tagged by a `BuiltinShapeId`: keywords in position, and at each slot a role beside one type per overload of the bucket, with one return apiece, plus the binder facts and the reserved bit each entry's readers ask for. The untyped bucket key is an erasure of that run, not a column of its own, and each slot's role says how the shape builder reads its part: as a written quote, as bare syntax, as a container of quotes, or evaluated. A node probes that table once, and every later reader names a shape by its tag rather than respelling its key.

`KExpression` is a `Copy` handle: its parts run and every string in it borrow the program storage the parse wrote them into. The scheduler dispatches a separate [`WorkingExpression`](src/values/working.rs), which is where a resolved sub-result gets spliced back in — so an expression *value* can never carry one. A node only reaches the value channel wrapped in the [program-storage marker](src/parse/ast/program.rs), which types the tier the channel's verdicts assume. See [src/parse/README.md](src/parse/README.md).

### load — `KExpression` → `BodyShape`, typed

[scope](src/scope/README.md) builds each body's shape: its statements rewritten,
its names classified into mentions, its components and the order its units run
in. [elaborate](src/elaborate/README.md) types every type expression, binder and
callable where the program loads, as handles in the
[type lattice](lattice/src/types/README.md), and dispatch's static selection narrows each
keyworded use's candidates and checks returns, ascriptions and `EVAL`s. A load
that refuses writes `error: <message>` and runs nothing.

### run — the drain

[program](src/program/README.md) holds the loaded program as one owning value,
and its body runner performs a body's units in order. Each unit is work on the
[scheduler](src/scheduler/README.md)'s drain, where a unit of work is a
[cellgraph](cellgraph/README.md) cell and every value it makes lives in that
cell's region ([values](src/values/README.md); functions, modules and circular
data are [knots](src/knot/README.md)). Dispatch's evaluator is the step every
evaluation runs: it reads a node off its shape, evaluates its slots, ranks the
candidates by the arguments' types and runs the one that wins. An error the
program does not catch ends the run.

## Source layout

The crate's top-level modules are [memory/](src/memory) (where a value lives
and how long — the cell tier and program storage, with `lattice`'s bump tier
re-exported), [parse](src/parse.rs) (text →
`KExpression`, plus the AST and builtin shape table that output is written in),
[values/](src/values.rs) (the data values and the per-dispatch expression form,
laid down in a cell's region — see [src/values/README.md](src/values/README.md)),
[scope/](src/scope.rs) (lexical environments: the shape a body owns its
statements and resolves its names through, closure bindings and activations — see
[src/scope/README.md](src/scope/README.md)),
[elaborate/](src/elaborate.rs) (type expressions elaborated into lattice handles
where they are read — see [src/elaborate/README.md](src/elaborate/README.md)),
[knot/](src/knot.rs) (functions, quotes, modules and circular data as values:
the node each one is, the tie that births a component as one knot, the doors
that birth a lambda or a quote no binder names, and the copy that re-ties it — see [src/knot/README.md](src/knot/README.md), and
[src/knot/module/README.md](src/knot/module/README.md) for the views `:|` and
`:!` build, the coercion that births a view's members, and the binding a
`USING … SCOPE` block enters on),
[scheduler/](src/scheduler.rs) (the deferred-work drain, where a unit of work is
a `cellgraph` cell — see [src/scheduler/README.md](src/scheduler/README.md)),
[program/](src/program.rs) (a loaded program as one owning value — program
storage, the interner, the type registry and the cell graph over them — and the
body runner that performs it — see
[src/program/README.md](src/program/README.md)),
[dispatch/](src/dispatch.rs) (the language koan's programs run under: the
builtin table and each builtin's type rule, the evaluator and keyword selection,
the overlap check, and static selection — see
[src/dispatch/README.md](src/dispatch/README.md)), and two re-exports of the
[lattice](lattice/README.md) crate: `symbols` ([the symbol
vocabulary](lattice/src/symbols/README.md)) and `type_lattice`, which is
`lattice::types` ([the closed algebra over interned type
nodes](lattice/src/types/README.md)). `parse` splits into [ast/](src/parse/ast.rs) (the syntax types, the node cache and the
eternal-tier program marker), and
[builtin_shapes/](src/parse/builtin_shapes.rs) (`BUILTIN_SHAPES` and the role /
binder / slot-layout facts riding its entries).

```
src/
├── main.rs              the interpreter binary — reads a program from a path or stdin, loads it under dispatch's Koan with stdout / stderr sinks, runs it, and exits non-zero on an uncaught error
├── lib.rs               library facade — declares every module and re-exports `lattice`'s symbols and types (the latter as type_lattice), so the binary and the tests reach the interpreter through one module graph
├── tests.rs             `#[cfg(test)]` crate-wide test scaffolding — installs audit/'s counting global allocator for the lib-test binary and exposes the tally an allocation bracket reads; tests/boundary.rs is the source scanner every module's boundary test hands its import lists to
├── source.rs            source-span and provenance carrier for errors
├── memory.rs            pub mod memory — where a value lives and how long: the cell tier over cellgraph and program storage, with `lattice`'s bump tier re-exported
├── memory/
│   ├── substrate.rs        the crate's only import of cellgraph — a re-export block binding the liveness width once (WIDTH), with the width-bound Ready / Operand / CellGraph / StepContext aliases
│   ├── slots.rs            SlotArray / SlotView — a fixed run of two-state write-once binding slots in a cell's region, safe code over cellgraph's once-written run: the invariant array binds, the covariant view reads
│   ├── knot.rs             Knot / KnotPlan / Edge / Member — a group of values that refer to each other, one run of index-edged nodes in a cell's region, tied once from a plan
│   └── program.rs          ProgramStorage / ProgramBrand — outside the graph, the one cellgraph Storage program text, the parsed AST, the builtin table and the program record are all written into at 'graph, through the brand's Writer
├── parse.rs             pub mod parse — the parser and what it produces: the syntax AST and the builtin shape table, written in the symbol vocabulary
├── parse/
│   ├── lower.rs            layout tree → KExpressions: sigils, marks and `#[…]`/`#{…}` element quoting, the layout-line peel, adjacency, spans
│   ├── atom.rs             classify one atom — the colon split, compound-operator desugaring
│   ├── brace.rs            DictFrame state machine for `{k: v}` / `{x = 1}` pairing
│   ├── operators.rs        operator registry
│   ├── ast.rs              the syntax AST: KLiteral / ExpressionPart / KExpression — Copy handles over written slices, with a Type part carrying only its TypeSymbol
│   ├── ast/
│   │   ├── shape.rs        PartClass / DispatchShape / KeyElement / ExpressionKey + NodeCache, the one structural cache both node families carry (stored key, shape, BUILTIN_SHAPES entry, binder plan) and the readers that fill it
│   │   └── program.rs      ProgramExpression / ProgramNode — the eternal-tier marker that makes "this node's parts run is hosted in program storage" a type
│   ├── builtin_shapes.rs   BUILTIN_SHAPES — every builtin bucket as one typed run spelled once, tagged by BuiltinShapeId, carrying each slot's role and its type per overload, one return per overload, its binder facts and its reserved bit; the bucket key derived off that run, the two build-time laws over it, KEYWORDS, and the single table probe
│   └── builtin_shapes/
│       ├── role.rs         Role / Reading / BodyKind / Heads / DefinitionKind — what each part of an entry is to name resolution, and how the shape builder reads it
│       ├── binder.rs       BinderFacts and the structural extractors: which name and bucket key(s) a binder shape declares, read off the node's cached entry
│       └── layout.rs       SlotLayout — a body's value binders as a symbol-sorted run, computed where the shape is lexically fixed
├── scope.rs          pub mod scope — koan's lexical environments over values and types, in three tiers: the shape, closure bindings and the activation
├── scope/
│   ├── shape.rs          BodyShape — one body's own statements rewritten, its declared-name runs, classified mentions with their coordinates, capture layout, components, nested shapes, the group frame it was built under and the groups it holds, the expression shape a callable body sits in, the body each binder births and each LET binder's right-hand side, its registrations, bucket declarations and each keyworded use's candidate list, and for a quote's code shape its carried type, its refusal, its required keyworded holes and the names and keys each EVAL offers, in program storage; the write-once load-time type cells — each recorded TypeExpression, type binder, registration and callable body, a callable body's group levels, the lexical variables a body declares and the type captures a callable reads them through, and a code shape's typing refusal — the load pass fills; Position / Coordinate / Site and ShapeError
│   ├── shape/build.rs    the one shape builder: the claims pre-scan and group frames, the rewrite pre-pass, the binders pass, the mention walk with its eager/deferred state (a nominal construction's payload a constructor slot), nested bodies, arms and quote values' code shapes, the components pass, and the units pass that orders a body's units
│   ├── shape/build/rewrite.rs  the operator-run rewrite — fold left, fold right, unary and pairwise, the pairwise hoist into a synthesized block, and a != b as NOT (a == b), every node built through parse's own constructor and spanned at the source it was built from
│   ├── shape/build/locate.rs   where an error found in a statement points — the part it is about, else the nearest spanned part or node — searched for on the error path only
│   ├── groups.rs         operator groups — the four builtin groups, the position-blind claims pre-scan over all the code being built, the GroupFrame chain deciding where a declared group is visible, and the cover one symbol chains under
│   ├── signature.rs      what a callable's signature and FOR ALL group declare for its body
│   ├── typed.rs          the load-time type vocabulary — Static (unknown, closed, or rigid over Variables a run supplies) and solutions, and the callable-typing records Callable / Registered / ParameterBinding and Elaboration, and the value channel's Statics and Narrowing
│   ├── builtins.rs       Builtins — the sorted builtin table every activation reads through its header, values then types, then the overloads grouped by bucket key
│   ├── closure.rs        ClosureBindings — a callable's captures, read from the enclosing activation into scratch then laid down: a Link, a value word or a knot edge, each; the run's copy and weight
│   └── activation.rs     ActivationView — one call's or block's Copy, Drop-free read half, covariant in its brand: its header, the knot member it runs and a view of its slots, read by coordinate (an edge capture as its sibling member); Activation — the view beside the slot array that binds, invariant, with one constructor per body kind
├── values.rs         pub mod values — Value, the 24-byte Copy sum over scalars, a region string, a borrow of each per-kind resident struct and a knot-member parameter; the Knotted / KnottedFamily trait pair and its vacuous Nothing / NoKnot default; ValueFamily / ValueCarrier; the text helper and the ascription retype
├── values/
│   ├── weight.rs         Weight — the saturating bytes a total rebuild writes, memoized on every composite
│   ├── type_value.rs     TypeValue — a type in value position beside its memoized OfKind type
│   ├── list.rs           List — one run of cells typed by the join of its elements, or a key's candidates typed `List<Any>`
│   ├── dict.rs           Dict / Key — sorted keys and aligned cells, a binary-search lookup, entry order key order
│   ├── record.rs         Record — symbol-sorted names and aligned cells, typed by the record of its fields
│   ├── tagged.rs         Tagged — the one nominal wrap: a payload under a type identity, constructed through the checked door, held or peeled
│   ├── link.rs           Link — a value word or an edge into the holder's own knot: a data node's cell, a closure binding
│   ├── circular.rs       Circular / Resolved / CodeView — a knot's data node over link cells, what a member holds as `values` reads it, and a data node's run listing and rebuild for its knot's copy
│   ├── admission.rs      satisfies over a value's memoized type, admits_part / part_ktype over a raw AST part, admits over a working part, and construction, the one newtype-construction rule, and representation, the type a tagged payload is read at
│   ├── surface.rs        Seen / Surface — the one door every read of a container or tagged value goes through: a value beside the type a read sees it at, opened to show only what that type names; Value::retyped
│   ├── crossing.rs       cross / cross_here over the placement doors, cross_view and copy_severed — the doors a copy comes through, the second for a value inside a copied operand of another family — the deep copy, and the crossing verdict
│   ├── working.rs        WorkingExpression / WorkingPart — the scheduler's per-dispatch node in the executing cell's region, carrying the parse's node cache
│   ├── equality.rs       Value::equals — structural equality, each side at the type it is seen at, containers gated on related seen types, a bisimulation over knot members — a function by its identity and captures, a quote by its syntax and bindings — Incomparable when a module or a barrier is reached
│   ├── render.rs         Value::render — the surface PRINT writes, a mark pass then a write pass labelling where a cycle closes
│   └── lower.rs          Value::lower_part — a region-pure AST part straight to a value
├── elaborate.rs      pub mod elaborate — type expressions elaborated into lattice handles where the program loads and, for what the load leaves unknown, through the activation they are read in
├── elaborate/
│   ├── expression.rs     type_expression — bare names, LIST OF, MAP ->, unions, record types, FN and EXPR types with their FOR ALL groups (refused outside a signature member), a signature's WITH application, a code kind NEEDING names, Union.Tag
│   ├── signature.rs      callable_type — a FN's, EXPR's or OP's type read off the expression shape its body sits in, with a registration's ranked shape and parameter binding
│   ├── channel.rs        type_channel — the load pass: every type binder, type expression, callable and registration — a USING block's surfaced head included — typed where the program loads, closed, rigid or unknown, into the shape's write-once cells
│   └── reads.rs          Reads / TypeAt — what elaboration reads names through: an activation, its view, or the load pass's reader
├── knot.rs           pub mod knot — functions, modules and circular data as values: the 16-byte Knotted member that closes Value's parameter, the Node it holds, the KValue / KActivation aliases, Supplied and Untieable, and the field a USING source names
├── knot/
│   ├── function.rs       Function — a function node: its memoized type, body shape, closure bindings and knot weight; the staging a tie does for a function member
│   ├── builtin.rs        BuiltinFunction — a builtin overload's node over its registered shape and native id, and the door that lays one down
│   ├── code.rs           Code — a quote's code node: its body, code shape, carried type and its bound and supplied runs; the quote door, the USING door, and the staging a tie does for a code node
│   ├── data.rs           a knot's data members: the staging walk with its anonymous nodes and evaluator by site, container memos by the nominal cut, the construction check, and the node write
│   ├── module.rs         Module — a module node and everything that reads one by name
│   ├── module/
│   │   ├── birth.rs          a module binder's activation and its tie once the body has bound every slot
│   │   ├── view.rs           the view door: what m :! Sig and m :| Sig build
│   │   ├── coerce.rs         members born coerced across an opaque view's barrier
│   │   ├── layout.rs         layout order: where a member sits in a module
│   │   └── surface.rs        entering a USING … SCOPE block: each surfaced name bound to its member
│   ├── tie.rs            tie — a component of value binders staged into scratch, memos derived and constructions checked, then laid down as one knot
│   └── copy.rs           the knot-member family's copy: a whole knot re-tied at the destination, edges verbatim
├── scheduler.rs      pub mod scheduler — the deferred-work drain over cellgraph's cells and liveness matrix: a unit of work is a cell, and this module adds the ready stack, the drain protocol and delivery, over one step bundle the layer above supplies
├── scheduler/
│   ├── drain.rs          Graph — a newtype over the CellGraph closed over the scheduler's families, with the storage-only slab roots it hands out and takes back; Scheduler — a per-call view over a borrowed Graph: the depth-first ready stack of live cells and unborn requests, run over one root work, the wake of a rested birth at the verdict's price, the deferred release a tail hand-off needs, a root work resumed from what an earlier one left (Resting), and DrainStalled; every birth and every death is the drain's
│   ├── action.rs         Step — the only thing a step is handed: its writers, its state and scratch state, spawn, results, and the park / tail / finish_fresh / finish_in_home / finish / done / leave / failed ends that alone build an Action (opaque, over the drain-only Kind); Placement, Use, Request, Received, Slot, the drain's Spawns buffer, StepError
│   ├── continuation.rs   StepBundle — the covariant birth family and its crossing, the parked state family, the scratch family and born, which the layer above supplies; BirthAt / StateAt, NativeStep, Work, and the ContinuationFamily a cell parks: Continuation, Rested, Provenance
│   └── delivery.rs       KDelivery — koan's delivery bundle: a scratch fill and a carrier fill, both the value family
├── program.rs        pub mod program — a loaded program as one owning value and the body runner that performs it, over elaborate, knot, memory, parse, scheduler, scope, symbols, type_lattice and values
├── program/
│   ├── record.rs         Program — the record a loaded program's steps read at 'graph, and evaluate, the one door every evaluation is asked through; Language — the builtin table, evaluator and shape check the layer above supplies; Output, Outcome, Contract, error values; Evaluated, LoadError
│   ├── bundle.rs         KBundle — koan's step bundle: the covariant KBirth (Program / Call / Eval / Evaluate / Block / Inspect), the parked KState, and the sites a parked runner keeps in scratch
│   ├── body.rs           run — the body runner, the one step that performs a body's units at the top level, in every frame and in a block, ending a frame under its contract or tailing its last statement; call and placement_of, the derived placement bit; block; eval and CodeRefused, the door that runs a quote's code
│   └── substrate.rs      CellSubstrate — program storage, the registry's bump and the interner as self_cell's owner, and Running — the graph, its root, the registry and the Program record at 'graph, with run and inspect, reached through a closure per call
├── dispatch.rs       pub mod dispatch — Koan, the Language programs run under, and the vocabulary its submodules share
└── dispatch/
    ├── builtins.rs       the builtin table — the lattice's types, Error and every overload as a builtin node — and the natives the overloads run
    ├── evaluate.rs       the evaluator step: what a node is, gathering its parts, an ascription, an EVAL, a keyworded call, an application, and finishing under a contract
    ├── select.rs         admission and selection over a candidate list, a keyworded call's argument record, and whether a call keeps a contract
    ├── check.rs          the overlap check: a user overload taking operands a builtin overload at its key already takes
    ├── statics.rs        static selection: a static type for every value expression and binder where the program loads, each keyworded use's candidates narrowed and chosen by them, and the return, ascription and EVAL checks
    └── errors.rs         the messages of the error values dispatch raises
```

The [lattice](lattice/README.md) crate's tree:

```
lattice/src/
├── lib.rs               the crate root — the import rule and `forbid(unsafe_code)` outside the test build
├── tests.rs             `#[cfg(test)]` crate-wide test scaffolding — installs audit/'s counting global allocator for this crate's test binary, the tally the heap-contract tests bracket, and the property-case share
├── bump.rs              pub mod bump — the bump tier: Bump, BumpAllocator (= &Bump), BumpVec, BumpBackedMap / BumpBackedSet and bump_table / bump_set; the crate's only import of bumpalo / hashbrown / allocator_api2
├── bump/
│   ├── components.rs       strongly_connected_components — Tarjan over an index graph, staged in a bump; the walk the type lattice's recursive groups and koan's scope bindings both condense by
│   └── scope_id.rs         ScopeId — counter-minted, position-independent scope identity for per-declaration types; an identity source, never looked up against
├── symbols.rs           pub mod symbols — Symbol, a name's 128-bit content digest, plus SymbolInterner (the run's digest→text side table, read only when rendering), the four classified wrappers, BindKind, the token classifiers and the identity hasher every symbol-keyed table uses; a leaf, so koan's parse and the type lattice rest on it rather than on each other
├── symbols/
│   └── tests.rs            interning laws, including how a static_name! records
├── types.rs             pub mod types — the closed algebra over interned type nodes: the vocabulary, the registry, the identity recipe, the relations and the unifier, over symbols and the bump tier and nothing else
└── types/
    ├── node.rs           TypeNode — one interned type's content, generic over the handle its children are read as; every child position is a handle, so a node is shallow; `view` reads one as another typed handle; Variable, the view of the three variable nodes and their two ends
    ├── handle.rs         Handle — the Copy content-digest handle — and the sealed typed handles over it: KType (concrete), Parametric, Scheme and DeclaredType; the pinned builtin constants, and the name/kind readings off one
    ├── run.rs            Run / Elements — typed views over a node's raw child runs
    ├── typed.rs          the typed relations the rest of koan calls: the order, join and meet over KType; fits, ranking, solving and substitution over parametric types and schemes
    ├── digest.rs         TypeDigest and the one identity recipe: the hand-written tag table, one layer deep, plus the schema and component digests
    ├── registry.rs       TypeRegistry — the region-hosted interning table (each node beside its probe flags), the composite doors, generic over the handle, canonical `union_of` reducing its concrete members, the binder doors `shape_scheme` and `function_scheme` keeping every variable, the checked conversion `concrete`, and the signature doors `signature`, `signature_apply` and `signature_meet`
    ├── verdicts.rs       VerdictTable — the fixed two-way cache of relation verdicts the registry lays in its region
    ├── kind.rs           KKind — the shallow kind a type-accepting slot admits
    ├── record.rs         Record — a Copy view over a region slice of BinderSymbol-keyed fields, backing record types and lambda parameter identity
    ├── shape.rs          DispatchTokenElement / DeferredReturnSurface / Specificity, the readers of a shape's parts (`Shape`), and the element and record rebuild helpers
    ├── operators.rs      ReductionMode / FoldDirection — how a run of a signature's operators reduces, which is part of the signature's identity
    ├── schema.rs         SigSchema over symbol-sorted Members tables, the SchemaDraft the signature door canonicalizes, and the channels' canonical orders
    ├── walk.rs           Variance and the two drivers every structural recursion goes through
    ├── walk/unary.rs     the arm table behind `visit` and `rebuild`, with the union door, the position context, and `visit_free_quantified`
    ├── walk/binary.rs    the pairing table behind `lockstep`: width verdicts, the variance flip, and the rebuild door
    ├── order.rs          is_subtype_of — the order, which never solves — and fits, the relation a question reads, as one Lockstep instance differing at its leaf; satisfied_by
    ├── lattice.rs        join (subsumption-or-union, not a walk) and the meet (the rebuilding Lockstep instance, relating a variable by the rigid rule for the solver)
    ├── unify.rs          admits_with and the Collector: contributions solved to a pair of ends and bound at its least instance; the Interval a solve reports per variable
    ├── substitute.rs     the quantifier, level and head-parameter substitutions, instantiation and erasure, and a type read through intervals (bound_above among them)
    ├── signatures.rs     a signature type as a set of applications: the order between two sets and the meet the signature_meet door interns
    ├── sig_relations.rs  sig_fits and fits_application — *fits* over signature types, solving each unpinned head parameter — with their failure record, the binder relations admits_shape and admits_function, and shape_specificity
    ├── ranking.rs        priority classes — admit_by_class, the per-class verdict class_at_least the registry records, select_by_class, and judge_by_class's never/always/maybe over static types
    ├── window.rs         RecursiveGroupWindow and seal_group — the open/seal doors and the Tarjan component pass behind them
    └── render.rs         surface-syntax rendering — the one recursion written by hand, over the registry and the symbol interner
```

## Design and roadmap

A module's design doc is the `README.md` in its own source directory, linked
from that module's top-of-file comment. The kept modules carry theirs:

- [src/parse/README.md](src/parse/README.md) — the division of labour with
  `sexlex`, the borrowed splice-free AST and its structural cache, and the
  `BUILTIN_SHAPES` table every node is classified against at construction.
- [src/memory/README.md](src/memory/README.md) — the storage tiers, the
  one-place substrate alias layer, the slot array and the knot, and the
  drop-freeness the region discipline rests on.
- [src/values/README.md](src/values/README.md) — the data values: per-kind
  resident structs born through a `Writer`, the one lifetime a value borrows at, the type memo `satisfies` reads, weight and the crossing
  verb, dict key order, and working expressions.
- [src/scope/README.md](src/scope/README.md) — lexical environments: the three
  tiers, eager and deferred mentions and the visibility rule over them, the
  components a knot can tie and the order a body's units run in, the three
  channels and unshadowable builtins, keyworded uses and their candidate lists
  and rankings, write-once slots, and the operator groups a body's statements
  are chained under.
- [src/elaborate/README.md](src/elaborate/README.md) — type expressions
  elaborated into lattice handles through the activation they are read in, and
  why one did not.
- [src/knot/README.md](src/knot/README.md) — functions, modules and circular
  data as values: the knot a component is tied into, what a birth may name, and
  the copy that re-ties a whole knot at its destination.
- [src/knot/module/README.md](src/knot/module/README.md) — modules as values:
  layout order, the views `:|` and `:!` build, the coercion that births a view's
  members, and the binding a `USING … SCOPE` block enters on.
- [src/scheduler/README.md](src/scheduler/README.md) — the deferred-work drain:
  the depth-first ready stack and the root work, what a step may name, the
  placement and use hints, delivery, the continuation and the step bundle, how
  a cell waits, and the tail hand-off.
- [src/program/README.md](src/program/README.md) — a loaded program: the owner
  and its dependent, the program record and the `Language` above it, koan's
  step bundle, the body runner that performs the top level and every
  called body, frames' contracts and tails, and error values.
- [src/dispatch/README.md](src/dispatch/README.md) — the language koan's
  programs run under: what a node is, the builtin table, selection by priority
  class, tails under a contract, errors, and the overlap check.
- [lattice/README.md](lattice/README.md) — the symbol vocabulary, the type
  lattice and the bump tier as one crate below koan, its import rule, the
  component walk and `ScopeId`; with
  [lattice/src/symbols/README.md](lattice/src/symbols/README.md) for why a
  symbol's identity is a content digest, why the interner is not a lookup
  authority, and what a symbol's binding class buys, and
  [lattice/src/types/README.md](lattice/src/types/README.md) for the closed
  algebra: its invariants, and an index of its parts — identity, the node
  vocabulary, the relations, solving, and [the laws](lattice/src/types/laws.md)
  with what breaks without each.
- [sexlex/README.md](sexlex/README.md) — the layout half of the parser: what it
  decides, the three things it refuses, and the three indentation regimes.
- [cellgraph/README.md](cellgraph/README.md) — the cell substrate's contract and
  verbs, with [cellgraph/src/graph/README.md](cellgraph/src/graph/README.md) for
  matrix liveness, the sealed tier and pricing, and
  [cellgraph/src/tree/README.md](cellgraph/src/tree/README.md) for tree cells.

A design no single module owns sits in [design/](design/gradual-typing.md):
[gradual typing](design/gradual-typing.md) follows a type from its declaration,
through the load, to a call, [quantified types](design/quantified-types.md)
says where a `FOR ALL` may be written and what stands in for a higher-ranked
type, and [modules](design/modules.md) says what a module's types are and when
two are one.

Future work lives in [roadmap/](roadmap/) — one file per work item, with `Requires:` /
`Unblocks:` cross-links. Its [README](roadmap/README.md) groups work into project
subdirectories — each with its own README naming the project and listing its ready-to-start
items — and derives a "Next items" list, everything with no still-open prerequisite, from
those cross-links (`tools/doclinks.py sync-next`).

The [cellgraph/](cellgraph/README.md) cell substrate carries its design in its
own module READMEs and its open work in a roadmap tree of its own, so it reads as
a standalone library rather than as Koan's internals; work items cross-link
across the trees and `doclinks` gates them as one dependency graph, but each tree
derives its own "Next items" list.

[sexlex/](sexlex/README.md) is the second crate Koan embeds: the layout half of
the parser, with no vocabulary of its own. Unlike cellgraph it carries no
roadmap tree — its README is the whole design statement, and the crate doc on
[sexlex/src/lib.rs](sexlex/src/lib.rs) is the precise form of the five rules
about whitespace, brackets, quotes, adjacency and indentation it rests on.

[lattice/](lattice/README.md) is the third: the symbol vocabulary, the type
lattice and the bump tier, so the lattice's closure — it names nothing of koan
— holds by the crate edge. Koan re-exports its modules under `crate::symbols`,
`crate::type_lattice` and `crate::memory`, so no koan file outside `lib.rs` and
`memory.rs` names the crate. Like sexlex it carries no roadmap tree: its open
work stays in koan's.
