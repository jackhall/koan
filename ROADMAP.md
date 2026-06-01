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
- *FUNCTOR binder.* A dedicated `FUNCTOR` binder with its `:(FUNCTOR (params) -> R)`
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
  into the fused dispatch walk, which reuses them instead of allocating fresh sub-Dispatches.
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
  wake sources with earliest-index-visible parking. A self-reference inside
  a keyworded field sigil (`STRUCT Tree = (children :(LIST OF Tree))`) is
  pre-resolved to a `RecursiveRef` carrier by `rewrite_threaded_self_refs`
  before the sub-Dispatch, so it lowers to `List(RecursiveRef("Tree"))`
  instead of deadlocking on its own placeholder.
- *Unified walk + strict-only admission.* Each `run_dispatch` builds a
  per-call `bare_outcomes` cache (one `NameOutcome` per bare-name part)
  shared between the resolver's strict admission and the fused
  splice / park / eager-sub walk, so each bare name resolves exactly
  once per call. Strict admission reads cached `Resolved` outcomes via
  `KType::accepts_part`, while `Parked` and `Unbound` fall back to
  shape-only admission so the post-pick walk can surface precise per-slot
  diagnostics. When no bucket admits, a post-walk fallback reads the cache
  by fixed precedence — placeholders > eager > unbound > pending overload
  > Unmatched — and `is_more_specific_than` ranks concrete carriers above
  the unconstrained-name slot types (`Identifier` / `TypeExprRef`) so an
  `ATTR <s:Struct>` overload beats an `ATTR <s:Identifier>` fallback.
  No-keyword shapes (`BareIdentifier`, `BareTypeLeaf`,
  `ConstructorCall`, `FunctionValueCall`, `SigiledTypeExpr`,
  `LiteralPassThrough`) ride dedicated fast-lane handlers that never
  enter the candidate walk. With `LiteralPassThrough` covering
  single-part literal-shaped expressions (`(99)`, `("x")`, `([1 2 3])`,
  `((inner))`), the fast-lane axis is exhaustive over keyword-free
  expressions: `Keyworded` ⟺ at-least-one keyword.
- *Direct constructor dispatch.* `STRUCT` and `UNION` constructions
  route through `execute::dispatch::constructors` directly — no
  registered `struct_construct` / `tagged_union_construct` primitives,
  no `BodyResult::Tail` re-dispatch through the Keyworded bucket. The
  `ConstructorCall` fast lane (leaf-Type head) and the
  `FunctionValueCall` fast lane (Identifier head resolving to a
  `KTypeValue(UserType{..})` alias) both dispatch into
  `constructors::dispatch_construct_struct` / `dispatch_construct_tagged`,
  which read the field / variant schema straight from the
  `UserTypeKind` identity, stage value-cells as per-slot eager
  sub-Dispatches, and call `struct_value::construct` /
  `tagged_union::construct` directly. The parked-on-eager-subs case
  rides `CtorState`'s resume arm.
- *Stateful dispatch driver.* Every `DispatchShape` variant runs on the
  state-bearing `run_dispatch` driver, which is now the sole dispatch
  body. The carrier shape (`DispatchState` enum + per-variant
  structs), `recent_wakes` wake-attribution side-channel, six
  fast-lane variants, the `Keyworded` variant with its eager-subs /
  bare-name-park / overload-park tracks, and the `FunctionValueCall`
  fast lane with its eager-subs / head-placeholder tracks all carry
  progress in the slot and advance by one edge per callback (no
  per-wake reclassification, no `bare_outcomes` rebuild, no
  `NodeWork::Bind` spawn on any dispatch path). The stateful resume /
  install-time short-circuit invocations go through an
  `invoke_to_step_pinned` helper that holds a sibling clone of
  `active_frame` across the call so `try_reset_for_tail`'s
  uniqueness check refuses the reset that would otherwise deallocate
  the arena `scope` lives in. A per-slot reserve frame ping-pongs
  across `NodeStep::Replace` so iteration 3+ of a recursive
  eager-subs resume swaps a two-iteration-old reserve into
  `active_frame` and tail-reuses *it* instead of allocating fresh
  — recovering the per-iteration `CallArena` shell allocation the
  pin-only shape would otherwise pay.
- *Per-call arena protocol doc.* [design/per-call-arena-protocol.md](design/per-call-arena-protocol.md)
  is the single named owner of the `Rc<CallArena>` contract — carriers, the
  lift-time anchor decision, the `alloc_object` cycle gate, active-frame
  propagation, the `outer_frame` chain for builtin-built frames, TCO frame
  reuse, and the ping-pong reserve rotation. The five docs that previously
  carried fragments (memory-model, execution-model, error-handling,
  typing/functors, typing/modules) keep their topic-specific narrative and
  cross-link the protocol page for the mechanics.
- *Dispatcher / scheduler facade.* The dispatch tree lives at
  `execute::dispatch`, sibling of `execute::scheduler` (and
  `execute::interpret` / `execute::lift`). Every dispatch entry point
  takes [`&mut DispatchCtx<'a, '_>`](src/machine/execute/dispatch/ctx.rs) —
  a newtype over `&mut Scheduler<'a>` exposing exactly the scheduler
  operations the dispatcher uses (slot queries, `DepGraph` mutations,
  sub-submission, the recent-wakes side-channel, list/dict-literal
  scheduling, plus the dispatcher-only `build_bare_outcomes` /
  `install_eager_subs` / `replace_with_parked_dispatch` /
  `resume_eager_subs` / `invoke_to_step{,_pinned}` ops). `DispatchCtx`
  also implements [`SchedulerHandle`](src/machine/core/kfunction/scheduler_handle.rs),
  so builtin sub-slot routing inherits the dispatcher's contextual
  frame/chain via the facade rather than re-borrowing the bare
  scheduler.
- *Type-only nominal identities.* `STRUCT` / `UNION` / `MODULE` / `Result`
  declarations write only `bindings.types`: each per-declaration
  `KType::UserType` identity carries its own schema payload
  (`UserTypeKind::Struct { fields }`, `Tagged { schema }`,
  `TypeConstructor { schema, param_names }`, alongside the existing
  `Newtype { repr }`), and construction reads that schema from the type
  entry rather than a value-side carrier. The `KObject::StructType` /
  `TaggedUnionType` carrier variants are gone, so `bindings.data` holds
  only runtime instances; value-position references synthesise
  `KTypeValue(identity)` on demand via `coerce_type_token_value`, and
  recursive types ride a cycle-close pre-install plus a schema-bearing
  upsert at finalize. `SIG` followed the same path by merging its
  constraint variant (`SatisfiesSignature`) and value variant
  (`Signature(s)`) into one `KType::Signature { sig, pinned_slots }` —
  disambiguated by position — so it writes a single type-side identity and
  the `register_nominal` / `try_register_nominal` / `derive_nominal_identity`
  machinery deleted. No nominal binder dual-writes; the type-language /
  value-language partition is total.

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
resolution, recursive-binder-submission, type-language-via-dispatch,
walk-unification, keyworded self-recursion, and positional-type-surface
retirement phases have shipped (see "What's shipped so far"); the remaining
item adds the user-functor application surface:

- [User-defined TypeConstructor keyworded application](roadmap/dispatch_fix/user-defined-typeconstructor-keyworded-application.md)

### Editor tooling — [roadmap/editor_tooling/](roadmap/editor_tooling/)

Surface that lets external tools — editors, debuggers, build systems — see
intermediate Koan state. The build-time / run-time scheduler split is the
foundation:

- [Two-phase execution: build-time with pegged inputs, run-time resume](roadmap/editor_tooling/two-phase-execution.md)
- [Continue-on-error for the REPL and batch mode](roadmap/editor_tooling/continue-on-error.md)
