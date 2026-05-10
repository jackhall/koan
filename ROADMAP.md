# Roadmap

Open structural items that don't fit in a single PR. Each entry below names the problem,
why it matters, and possible directions — not a fixed design. Per-item write-ups live in
[roadmap/](roadmap/).

The order matters. Sequencing is purely about technical and design dependencies — Koan has
no users yet, so backward-compatibility costs play no role. The cost being optimized is
engineering rework: doing one item before another it depends on means doing the dependent
item twice. Each per-item file ends with a **Dependencies** section linking to its
prerequisites and the items it unblocks.

Design rationale for what's already in the language lives in [design/](design/) — six
topical docs covering the execution model, memory model, functional programming, type
system, expressions and parsing, and error handling. Two further design docs capture
cross-cutting work in flight: [design/module-system.md](design/module-system.md) — the
module-based abstraction system end-to-end (stage 1 shipped, remaining stages tracked
as `module-system-*` roadmap items below) — and [design/effects.md](design/effects.md)
— in-language monadic side effects (implementation tracked in
[roadmap/monadic-side-effects.md](roadmap/monadic-side-effects.md)). What's
shipped so far: user-defined functions, the dispatch-as-node scheduler refactor,
first-cut tail-call optimization, the leak fix (with lexical closures + per-call
arenas), structured error propagation, the user-defined-types substrate (return-type
enforcement at runtime), the IF-THEN→MATCH consolidation (`MATCH` accepts `Bool`
directly via projection at entry), per-parameter type annotations on user-fn
signatures, container type parameterization (`List<T>`, `Dict<K, V>`,
`Function<(args) -> R>`), transient-node reclamation (Bind/Combine sub-trees
recycled via a per-slot deps sidecar + free-list, keeping repeated-call scheduler
memory near-constant), per-call-frame chaining for builtin-built frames (MATCH's
child-scope `outer` no longer dangles when a TCO replace drops the call-site frame),
a targeted KFuture lift anchor (an addresses-only side-table on `RuntimeArena`
answers a precise membership query, replacing the previous always-anchor conservative
path), the leak-fix audit sign-off (a cycle gate on per-call `alloc_object`
redirects self-anchored values to the outer arena, closing out the audit slate at 0
leaks and 0 UB under Miri tree borrows), the quote/eval sigils (`#(expr)` and
`$(expr)` — surface forms that capture an AST as a `KExpression` value or evaluate a
`KExpression` value as code, closing the gap between "`KExpression` is first-class"
and "user code can manipulate expressions ergonomically"), the module-system
stage 0 cleanup (vestigial `KType::TypeRef` removed in favor of the unified
`TypeExprRef` slot kind, struct values now `IndexMap`-backed so PRINT emits fields
in declaration order, constructor dispatch funneled through a single
`dispatch_constructor` helper, and a `TypeResolver` trait threaded through
`KType::from_type_expr` ready for stage 1's module-aware resolver), and the
module-system stage 1 module language (`MODULE` and `SIG` declarators bind
structures and signatures under Type-token names; `:|` opaque ascription mints
fresh `KType::ModuleType { scope_id, name }` per declared abstract type so two
ascriptions of the same source module are observably distinct types; `:!`
transparent ascription shape-checks against the signature without re-tagging
identity; `Module`/`Signature` first-class values arena-allocated alongside
`KFunction` and reachable via `Foo.member` ATTR access), and the lift-walk
and aggregate-scheduler dedup (a single `any_descendant` predicate-walker
serves both `needs_lift` and `kobject_borrows_arena`; list- and dict-literal
planning collapsed into a single `Combine` scheduler variant whose
host-side `finish` closure captures the construction logic and folds
already-resolved literal scalars in alongside dep results; module/signature
resolution lives next to the `Module` / `Signature` types and serves both
ascription operators and `MODULE_TYPE_OF`), and the dispatcher extraction (overload resolution lifted
out of `Scope` into a dedicated `dispatcher.rs` of free functions taking
`&Scope`; `Scope::dispatch` and `Scope::lazy_candidate` are now thin
forwarders so `scope.rs` is back to lexical-environment storage and direct
mutators only), and the `KType` concern split (the 694-LOC `ktype.rs`
partitioned into three sibling files — core enum plus `name()` rendering
in `ktype.rs`, dispatch-time predicates in `ktype_predicates.rs`, and
name/type-expression elaboration plus `join` in `ktype_resolution.rs`),
and dispatch-time name placeholders (binders install a `name → producer
NodeId` entry in a new `Scope::placeholders` sidecar at dispatch time;
bare-identifier slot lookups whose target binder has dispatched but not
yet executed park on the producer via the existing `notify_list` /
`pending_deps` machinery instead of failing with `UnboundName` — see
[design/execution-model.md § Dispatch-time name placeholders](design/execution-model.md#dispatch-time-name-placeholders);
same-scope rebind of a value name now surfaces as a structured `Rebind`
error and an exact-signature `FN` overload conflict as `DuplicateOverload`).
The next
signature revision after error handling lands monadic side-effect capture; the
type-system arc runs through the module-system stages — foundation now landed
in stage 1, ergonomic generic dispatch in stage 5, coherence in stage 6.

## Next items

Items with no unresolved roadmap-level prerequisites — any of these can be picked up
without first landing something else:

- [Stage 2 — Module values and functors through the scheduler](roadmap/module-system-2-scheduler.md) —
  make module expressions, type expressions (with incremental refinement), and
  functors full participants in the scheduler's free-execution model; carries
  forward the post-stage-1 Miri audit slate.
- [Per-declaration type identity for structs and tagged unions](roadmap/per-declaration-type-identity.md)
  — extend the `KType::ModuleType` per-declaration identity carrier to `STRUCT` and
  `UNION` so two distinct declarations report distinct types.
- [Files and imports](roadmap/files-and-imports.md) — wire `.koan` files together so a
  codebase can span more than one source file and files become modules.

## Open items

### Memory and runtime substrate

- [Generalize `Scope::out` into monadic side-effect capture](roadmap/monadic-side-effects.md)
  — replace the ad-hoc `Box<dyn Write>` with an in-language `Monad` signature
  (see [design/effects.md](design/effects.md)) plus a runtime `Effectful<T>` carrier;
  ships standard effect modules (`Random`, `IO`, `Time`). Requires module-system
  stage 2's functor support so the `Wrap` slot can be higher-kinded.

### Module system

The agreed design is captured in [design/module-system.md](design/module-system.md);
stage 1 shipped (the module language: `MODULE`/`SIG` declarators, `:|`/`:!`
ascription, per-module type identity), and the remaining stages below land
the rest incrementally, each producing a usable end state.

- [Stage 2 — Module values and functors through the scheduler](roadmap/module-system-2-scheduler.md) —
  make module expressions, type expressions (with incremental refinement), and
  functors full participants in the scheduler's free-execution model; carries
  forward the post-stage-1 Miri audit slate.
- [Stage 4 — Property testing and axioms](roadmap/module-system-4-axioms-and-generators.md)
  — Rust-side property-testing engine kept disjoint from dispatch; axiom syntax in
  signatures with compile-time checking on ascription.
- [Stage 5 — Modular implicits](roadmap/module-system-5-modular-implicits.md) —
  implicit module parameters with lexical resolution and strict-on-ambiguity.
- [Stage 6 — Equivalence-checked coherence](roadmap/module-system-6-equivalence-checking.md)
  — cross-implicit equivalence testing; the differentiating coherence story.
- [Stage 7 — Syntax tuning and witness types](roadmap/module-system-7-syntax-tuning.md)
  — disambiguation sugar designed against patterns from real stage-5 code, plus opt-in
  witness types.

### Type system

- [Group-based operators](roadmap/group-based-operators.md) — `+`/`-` form a math group
  but the language treats every operator as a flat independent builtin. Generic
  dispatch over groups arrives with the module system's modular implicits.
- [Per-declaration type identity for structs and tagged unions](roadmap/per-declaration-type-identity.md)
  — `KType::Struct` and `KType::Tagged` are flat singletons, so two distinct
  `STRUCT` declarations report the same type. Extend per-declaration identity
  along the lines of the module system's `KType::ModuleType` carrier.
- [Uniform §7 / §8 handling for Type-tokens in value slots](roadmap/type-token-auto-wrap.md)
  — `classify_for_pick` carves Type-tokens out of the §7 wrap rule for `Any` /
  `TypeExprRef` slots and never §8-parks them; collapsing the carve-out
  hits a chained-Lift + §8-park scheduler deadlock that needs diagnosis
  before the Identifier-vs-Type dispatch paths can unify.

### Surface and ergonomics

- [Files and imports](roadmap/files-and-imports.md) — a Koan codebase is one file;
  no way for a `.koan` file to reach into another, and no story for how files become
  modules.
- [Error-handling surface follow-ups](roadmap/error-handling.md) — errors-as-values,
  source spans on `KExpression`, continue-on-error (independent), plus typed
  user errors and the catch surface (gated on module-system stage 2).
- [Standard library](roadmap/standard-library.md) — collections (`Set`, `Map`,
  …) and standard effect modules (`Random`, `IO`, `Time`) ship as Koan-source
  functor FNs across multiple `.koan` files; doubles as the canonical example
  of idiomatic module / signature / functor / import composition.

### Future-facing

- [Static type checking and JIT compilation](roadmap/static-typing-and-jit.md) — the
  tooling and performance ceiling; both want a phase between parse and execution.
