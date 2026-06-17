# Roadmap

Open structural items that don't fit in a single PR. Each entry below names the problem,
why it matters, and possible directions — not a fixed design. Per-item write-ups live in
this directory, one file per item.

The order matters. Sequencing is purely about technical and design dependencies — Koan has
no users yet, so backward-compatibility costs play no role. The cost being optimized is
engineering rework: doing one item before another it depends on means doing the dependent
item twice. Each per-item file ends with a **Dependencies** section linking to its
prerequisites and the items it unblocks.

Design rationale for what's already in the language lives in [design/](../design/) — five
topical docs covering the execution model, memory model, functional programming,
expressions and parsing, and error handling, plus [design/typing/](../design/typing/README.md)
covering the type and module systems end-to-end.

What's shipped that the open items below build on:

- *Operator-chain substrate.* Pure-symbol tokens that aren't builtin compound triggers
  classify as keywords, and [`KExpression`](../src/machine/model/ast.rs) caches a
  `DispatchShape` at parse time — including an `OperatorChain` track for the slot-led
  `Slot (Keyword Slot)+` shape, with its sorted-joined operator probe. A per-scope
  operator registry (`Bindings::operators`, walked by
  `Scope::resolve_operator_group_with_chain` like every other name) resolves a chain's
  probe to a shared `OperatorGroup`, and the `OperatorChain` dispatch arm hits that
  registry — missing cleanly on an undeclared or cross-group mix, or reaching the reduction
  seam on a hit. The reducer itself and the `GROUP`/`OP` declaration surface are the remaining
  open work under
  [user-definable n-ary operators](operator_chaining/n-ary-operators.md) and
  [user-defined operator modules](operator_chaining/user-defined-operator-modules.md).
  See [design/expressions-and-parsing.md § Structural cache and dispatch shape](../design/expressions-and-parsing.md#structural-cache-and-dispatch-shape).
- *Immutable run-global root.* The builtins live once in a distinctly-typed
  (`ScopeKind::Root`) immutable root; a `RunScope` child takes top-level binds, and every
  scope carries a direct `root` reference. Builtins are unshadowable — a user type, FN/
  FUNCTOR overload, or operator colliding with a builtin is a `Rebind` at any depth — so a
  builtin (type, operator, or dispatch bucket) resolves root-first in one hop, ahead of the
  chain walk. See [design/typing/lookup-protocol.md § The immutable root and unshadowable builtins](../design/typing/lookup-protocol.md#the-immutable-root-and-unshadowable-builtins).
- *Anonymous functions.* A keyword-less `FN :{<field schema>} -> T = (body)`
  literal evaluates to a plain function value with no dispatch keyword, bound by
  `LET` or dropped into a function-typed slot — the record-schema sigil resolves
  to a `KType::Record` that a third `FN` overload's `TypeExprRef` signature slot
  admits. It makes the [standard library](libraries/standard-library.md)'s
  higher-order combinators ergonomic to call with an inline function. See
  [design/functional-programming.md § Anonymous functions](../design/functional-programming.md#anonymous-functions).
- *Duplication consolidation.* Six pre-located copy-paste clusters each collapsed to a
  single owner: per-builtin typed-binder `binder_name` (one shared
  [`type_part_binder_name`](../src/builtins.rs)), the FN and FUNCTOR bodies (one shared
  [`build_fn_like`](../src/builtins/fn_def.rs) keyed on `FnKind`), the `finish.rs` dep-finish/catch handler arms (one shared
  [`run_wait`](../src/machine/execute/run_loop.rs)), the `dict_literal`
  `accept_colon`/`accept_equals` pair (one `accept_separator`), the slot-extract error
  envelope (one [`require_kexpression`](../src/machine/core/kfunction/action.rs)
  owning the parenthesized-slot error text), and the scheduler `Object`/`Type` finalize
  arms (one [`check_declared_return`](../src/machine/execute/finalize.rs)
  parameterized over the lifted carrier's `matches_value`/`matches_type` predicate).
- *Arena unsafe consolidation.* Every captured/defining-scope re-attach is funnelled behind one
  [`ScopePtr`](../src/machine/core/scope_ptr.rs); `RuntimeArena::escape` is `NonNull`.
  The store-side erasure lives behind one sealed `ArenaStored` trait: all six
  arena-stored families route a single audited union-move `erase_store` and one gated
  `alloc` engine, replacing the six per-type `T<'a> → T<'static>` transmute pairs with
  one. The branded `ScopePtr<'a>` makes `Module::child_scope` and `Signature::decl_scope`
  safe re-attaches, concentrating the irreducible
  `'static → 'a` fabrication at the non-generic `CallArena` boundary.
  Honest slot storage landed for per-call frame scopes: a frame scope rides its slot as a
  payload-less [`NodeScope::Yoked`](../src/machine/execute/nodes.rs) marker re-projected from the
  slot's own `Node.frame` cart — no fabricated run-length `&'a` persists across a TCO reset.
  The frame re-anchor then landed in full: the free `&'run` scope fabrication at the read
  boundary is deleted, a within-step frame lifetime `'s` (`'a: 's`) threads
  `run_dispatch`/`SchedulerView`/`BuiltinFn`, and a slot's scope is now read on demand via
  [`Scheduler::current_scope`](../src/machine/execute/run_loop.rs) through the witness-bounded
  [`CallArena::scope_bounded`](../src/machine/core/arena.rs) brand (the post-step loop reads it
  through a `PostStep` token off the slot's returned frame). The sole surviving free re-exposure
  is the arena half of [`CallArena::with_anchored_child`](../src/machine/core/arena.rs), the
  C0-irreducible seed bind, and `KFunction::captured` now rides a `BoundedScopePtr`
  (see [design/per-call-arena-protocol.md § Slot-table scope handle](../design/per-call-arena-protocol.md#slot-table-scope-handle)).
  See [design/memory-model.md § Arena lifetime erasure](../design/memory-model.md#arena-lifetime-erasure).
- *Per-node output lift.* A node continuation's output is bound to the per-step frame
  lifetime `'s` ([`Outcome<'run, 's>`](../src/machine/execute/outcome.rs)), not `'run`. The
  producer keeps its terminal in its own frame (the slot's `Done` co-stores the backing
  `Rc<CallArena>`, so frame death moves Done→free) and does not lift; each consumer
  pull-lifts its deps into its own arena at read ([`run_wait`](../src/machine/execute/run_loop.rs))
  through the single [`NodeLift`](../src/machine/execute/lift.rs) workload hook, so an
  intermediate value dies with its consumer and only a consumer-less root drains to the run
  arena. Return-contract enforcement stays a separate Done-time layer. This output-lifetime
  shrink and lift hook were the prerequisite half of confining `'run` to `KoanRuntime`; the
  *workload-independent DAG runtime* that completes it has shipped — the scheduler is a
  crate-root [`mod scheduler`](../src/scheduler.rs) generic over a `Workload`, naming no Koan
  value, scope, memory, or AST type, with the Koan driver in
  [`execute::run_loop`](../src/machine/execute/run_loop.rs). See
  [design/per-call-arena-protocol.md § Consumer-pull node-output lift](../design/per-call-arena-protocol.md#consumer-pull-node-output-lift).
- *Unified erase/reattach carriers.* The hand-rolled erase-to-`'static` /
  reattach carriers (`ScopePtr`, `ErasedContract`, `ErasedCont`, the scheduler's `Erased<W::Value>`)
  and the cluster of one-off `outcome.rs` reference reattaches now share one generic
  [`Erased<T>`](../src/scheduler/erase.rs) owner over an `unsafe trait Reattachable { type
  At<'r>; }` lifetime-family. A single `retype` primitive (a `ManuallyDrop` `transmute_copy`) is the
  only lifetime-retype site; each carrier is a declarative `unsafe impl Reattachable` beside its own
  type (`ContractFamily`, `CarriedFamily`, `ContFamily`, `KObjectFamily`, `ScopeFamily`, …) with no
  `transmute` of its own, and the liveness witness moves to the call site. See
  [design/memory-model.md § Arena lifetime erasure](../design/memory-model.md#arena-lifetime-erasure).
- *Position-dependent type resolution.* Type names obey strict source order like the value
  language — a forward type reference is a position error — so the `nominal_binder`
  visibility carve-out is retired and `visible` is the single `idx < cutoff` rule across both
  languages. A nominal type is a member of an `Rc`-owned `RecursiveSet`: the external handle
  is `KType::SetRef { set, index }`, an intra-set sibling is `SetLocal`, and lift is
  `Rc::clone` of the whole cycle-aware group (replacing the per-declaration `UserType` tag).
  Mutual recursion of two or more types is co-declared with the `RECURSIVE TYPES Name = (...)`
  block; self-recursion threads the declaring name. See
  [design/typing/user-types.md](../design/typing/user-types.md) and
  [design/typing/elaboration.md](../design/typing/elaboration.md).
- *Plain-English type-operation surfaces.* The `type_ops.rs` underscore-keyword family is
  retired: a module type-member is the dotted `M.T`, `:(LIST OF T)` / `:(MAP K -> V)` replace
  `LIST_OF` / `DICT_OF`, and signature specialization is the infix `sig WITH {Slot = Type}`
  (record-literal bindings). Computed return types are bare tokens or `:(…)` / dotted
  `SigiledTypeExpr` carriers — the redundant parens-form `KType::KExpression` return overload is
  gone. See [design/typing/functors.md](../design/typing/functors.md) and
  [design/typing/ktype.md](../design/typing/ktype.md).
- *One bare-leaf type resolver.* The synchronous `coerce_type_token_value` is folded into
  [`resolve_type_leaf_carrier`](../src/machine/execute/dispatch/resolve_type_expr.rs) over the
  memoized, park-capable `Scope::resolve_type_expr` bridge, so a bare type-name leaf resolves
  through one cache and parks on an earlier still-finalizing binder like every compound type
  form; the dead paired-carrier-recovery branch is gone. The type/value binding partition is
  now total at the LET boundary — a type binds only under a Type-classified name, so
  `LET t = Point` under a value-classified name is rejected. See
  [design/typing/elaboration.md](../design/typing/elaboration.md).
- *Product-side nominal collapse.* A struct is a `NominalKind::Newtype` over a `KType::Record`,
  carried as `Wrapped { inner: Rc<KObject::Record>, type_id }`; the `STRUCT` declarator,
  `KObject::Struct` carrier, the `NominalSchema` / `ProjectedSchema` / `NominalKind` `Struct`
  triple, the `:Struct` wildcard, and `dispatch_construct_struct` are retired. The spelling is
  `NEWTYPE Name = :{fields}`; `.x` reads the field through ATTR's `Wrapped` fall-through over
  the record repr. A record repr threads its binder name, so a self-reference
  (`NEWTYPE Node = :{next :Node}`), a `:(LIST OF Self)` field, and `RECURSIVE TYPES` blocks of
  record newtypes all seal to `SetLocal` back-edges. The `:{…}` record type is now first-class:
  the parser emits a dedicated `ExpressionPart::RecordType` part that the elaborator folds
  straight to `KType::Record` (the internal `RECORD` type-constructor builtin and its desugar
  are retired), and a nested record field type elaborates inline so the outer binder threads in.
  See [design/typing/user-types.md](../design/typing/user-types.md).
- *Tagged-union variants as dispatchable types.* Each `UNION` variant is its own
  `KType::Variant { set, index, tag }` — a refinement reached through its union, keyed on
  `(set ptr, index, tag)` — so a variant value's `ktype()` reports the variant, a
  `:(Maybe Some)` slot dispatches on a single variant while `:Maybe` admits any, and a
  variant ≺ its union ≺ `OfKind(Tagged)`. Variant tags are now capitalized `Type` tokens (`Some`,
  `Ok` / `Error`), and the union-qualified `:(Maybe Some)` sigil names a variant type. The
  `Result` / `CATCH` / `TRY` error model keeps its `TypeConstructor` identity unchanged. The
  remaining `MATCH`-onto-type-dispatch lowering and recursive variant references are still open
  under [tagged-union variants as dispatchable types](type_language/tagged-variant-types.md).
  See [design/typing/user-types.md](../design/typing/user-types.md).
- *Types ride the value channel raw.* The scheduler's value currency is a two-arm sum
  [`Carried<'a> { Object(&KObject) | Type(&KType) }`](../src/machine/model/values/carried.rs)
  — a type-operator returns a raw `&KType` and a type argument arrives in the `Type` arm,
  retiring the `KObject::KTypeValue` / `KObject::TypeNameRef` transport boxes and their
  box/unbox round-trip at the binding seam. The bundle dual is `ArgValue` and an aggregate
  cell is `Held` (a list/dict/record element may be a first-class type). A deferred bare
  user-name leaf is now the `KType::Unresolved(TypeName)` transient (sibling to
  `RecursiveRef`), consumed by `Scope::resolve_type_expr`. Modules and signatures travel in
  the `Type` arm (projected by `require_module` / `require_signature`), and a shallow
  [`KKind` `{ Proper, Module, Signature, Any }`](../src/machine/model/types/kkind.rs) carried
  as `KType::OfKind` classifies a type at dispatch — absorbing the former `TypeExprRef` /
  `Type` / `AnyModule` / `AnySignature` `KType` markers. See
  [design/typing/elaboration.md](../design/typing/elaboration.md) and
  [design/execution-model.md](../design/execution-model.md).
- *Type-kind classification unfused from representation dispatch.* The two parallel kind
  classifiers fold into one subsumption lattice on `KKind`
  (`Any > {Module, Signature, Proper > {Tagged, Newtype, TypeConstructor}}`): the separate
  `NominalKind` enum and the `AnyUserType` wildcard `KType` variant are gone, and `kind_of`
  is the sole type→kind classifier, descending `SetRef` / `Variant` / `ConstructorApply` to
  report the nominal family. `OfKind(KKind)` is the one type-accepting slot and is
  **type-channel-only** — it admits a type value by `kind_of` subsumption, never a runtime
  instance. `ATTR`'s newtype field access matches its value through the least-specific `Any`
  slot and validates the `Wrapped` shape in `access_field`, and `CATCH`'s return is the
  documentary `:(Result Any KError)` rather than a nominal-family wildcard. See
  [design/typing/ktype.md](../design/typing/ktype.md) and
  [design/typing/user-types.md](../design/typing/user-types.md).
- *Dispatcher pulled out of the scheduler via a write-effect contract.* The dispatch tree
  mirrors the builtin `Action` / `run_action` split: a shape handler *decides* against a
  read-only view and *returns* its scheduler mutations as an
  [`Outcome`](../src/machine/execute/outcome.rs) effect that a
  [harness](../src/machine/execute/runtime.rs) interprets — so no dispatch handler
  holds `&mut Scheduler`. Eager-subs is modelled as the dispatcher's own dep-finish (the same
  N→1 shape the action harness installs): deps declared, the `Future`-cell splice lives in the
  finish, and the scheduler stays splice-unaware. A builtin invoked mid-dispatch routes through the shared action harness,
  reading the dispatcher's ambient frame/chain off the view.
  See [design/execution-model.md § The dispatcher / scheduler boundary](../design/execution-model.md#the-dispatcher--scheduler-boundary).
- *Unified scheduler interface.* Every scheduler-facing step — a dispatch decide, a finish, a
  builtin body, an invoke — decides against one read-only
  [`SchedulerView`](../src/machine/execute/dispatch/ctx.rs) and returns one AST-free
  [`Outcome`](../src/machine/execute/outcome.rs) (`Done` / `Continue` / `ParkThenContinue` /
  `Forward` — no variant names a `KFunction` or `KExpression`; the dispatch→execution hand-off
  folds into a dep-free `Continue` whose frame placement installs the per-call cart), with
  [`apply_outcome`](../src/machine/execute/runtime.rs)
  the sole graph writer. The `SchedulerHandle` trait, `BodyResult`, `DispatchOutcome`, and the
  per-shape `DispatchState` envelope are gone — the scheduler's write methods are inherent and
  private to the execute tree. A multi-statement FN body's leading statements are now owned deps
  the activation parks on, so they sequence and cascade-free before the tail reuses the frame —
  tail recursion with side-effecting statements runs in constant frame space. See
  [design/execution-model.md § The dispatcher / scheduler boundary](../design/execution-model.md#the-dispatcher--scheduler-boundary).
- *`NodeWork` carries no AST.* The scheduler's slot-work collapsed to a single
  [`NodeWork`](../src/machine/execute/nodes.rs) struct — one captured `SchedulerView -> Outcome`
  `cont` (the combine/catch/decide behavior built in by combinators; the `<deps>` dep-error label
  is harness policy) plus a pre-rendered deadlock-summary string. Binder-install and the recursive eager-sub pre-submission moved out of `Scheduler::submit_node`
  into a dispatch-layer [`submit_dispatch`](../src/machine/execute/dispatch/submit.rs) chokepoint,
  so the scheduler never introspects a `KExpression` — `submit_node` is a generic slot allocator
  and no `NodeWork` variant names an AST. See
  [design/execution-model.md § Dispatch birth and resume](../design/execution-model.md#dispatch-birth-and-resume).
- *Literal lowering hoisted to the dispatcher.* The aggregate-literal lowering
  (`schedule_list_literal` / `schedule_dict_literal` / `schedule_record_literal`, the `Slot`
  layout enum, `classify_aggregate_part`, `resolve_aggregate_bare_name`) moved from
  `scheduler/literal.rs` to [`dispatch/literal.rs`](../src/machine/execute/dispatch/literal.rs),
  next to the harness that drives it. It was the one file in the scheduler subtree that
  name-resolved and built values from an `ExpressionPart`, so the "scheduler names no AST"
  invariant now holds structurally across `scheduler/**`. The methods are `&mut self` on
  `KoanRuntime` (below), reached through the scheduler's public surface (`submit_in_own_scope` /
  `current_scope`). See [design/execution-model.md](../design/execution-model.md#the-dispatcher--scheduler-boundary).
- *Dep-request enum made AST-free at the source.* The six-arm dep enum a
  `ParkThenContinue` declares (`Dispatch` / `ListLit` / `DictLit` / `RecordLit` / `BodyBlock` /
  `Existing`) is renamed `DispatchDep`→[`DepRequest`](../src/machine/execute/dispatch.rs) and
  moved out of `outcome.rs` to the dispatch side, beside `PendingSub` — the layer that
  legitimately names AST. The data arms are unchanged (each still carries its `KExpression` /
  `ExpressionPart`), and the harness `match` still does every `&mut Scheduler` write. The win:
  `outcome.rs` imports no `crate::machine::model::ast` and carries `DepRequest` as an opaque
  type, so the decide phase stays read-only and the scheduler-step currency names no AST.
- *`KoanRuntime` owns the scheduler.* A
  [`KoanRuntime<'run>`](../src/machine/execute/runtime.rs) owns the `Scheduler` by composition
  (a `sched` field) and is the **sole** holder of `&mut Scheduler` across the execute tree. The
  execute loop, `apply_outcome` (the one graph writer), `submit_dispatch`, the aggregate-literal
  lowering, and the AST-aware submission wrappers (`enter_block`, `dispatch_in_own_scope`,
  `dispatch_in_active_frame`, `dispatch_body`, `submit_dep_finish_in_own_scope`) are all `&mut self`
  methods on it. `Scheduler` keeps the AST-free read views and low-level write primitives, so a
  dispatch decide sees only a read-only `SchedulerView` / `&Scheduler` — "everything outside the
  harness is read-only" is now structurally enforced by the type, not a naming convention. The
  `apply_outcome` cluster migrated up to `execute/runtime.rs` (the old `dispatch/harness.rs` is
  gone), unifying "the harness" at the `execute/` level above both `dispatch/` and `scheduler/`.

## Next items

Computed from the dependency graph: every roadmap item whose `Requires:` list no
longer names an unshipped item — anything here can be picked up without first
landing something else. Regenerated by `python3 tools/doclinks.py sync-next`; do
not edit by hand. Per-item descriptions live in the Open items subsections below.

- [Continue-on-error for the REPL and batch mode](editor_tooling/continue-on-error.md)
- [Files and imports](libraries/files-and-imports.md)
- [User-definable n-ary operators](operator_chaining/n-ary-operators.md)
- [Module system stage 5 — Modular implicits](predicate_typing/modular-implicits.md)
- [Memoized subtype matching](refactor/memoized-subtype-matching.md)
- [Merge the raw-type-part slot markers](refactor/merge-raw-type-part-slots.md)
- [Codebase-wide naming and responsibility audit](refactor/naming-and-responsibility-audit.md)
- [Node-lifetime lift and contract re-anchor](refactor/node-lifetime-lift-and-contract.md)
- [Content-addressed type identity](refactor/type-identity-registry.md)
- [Unify the type-resolution-outcome enums](refactor/unify-resolution-outcome.md)
- [Constructors as first-class function values](type_language/constructor-as-first-class-function.md)
- [SIG abstract vs manifest type members](type_language/sig-abstract-vs-manifest-types.md)
- [Tagged-union variants as dispatchable types](type_language/tagged-variant-types.md)

## Open items

Each subdirectory here is one project — a coherent body of work
whose items share design constraints and ship together. Per-item write-ups (problem,
impact, directions, dependencies) live in the subdirectory; the summaries below name
what the project buys the language and list its open items.

### Predicate typing — [predicate_typing/](predicate_typing/)

The user-facing typing stages — axioms, modular implicits, equivalence-checked
coherence, witness types — that ride on top of the type-language substrate.
The agreed design is captured in [design/typing/](../design/typing/README.md);
stages 1 and 2 shipped (the module language: `MODULE`/`SIG` declarators,
`:|`/`:!` ascription, per-module type identity, plus the scheduler-driven
elaborator, `WITH` sharing constraints, and higher-kinded type-constructor
slots, plus runtime type-parameter carriers on `List` / `Dict` / `Result`
values with ascription stamping at the FN return, argument, and `LET`
boundaries):

- [Stage 4 — Property testing and axioms](predicate_typing/axioms-and-generators.md)
- [Stage 5 — Modular implicits](predicate_typing/modular-implicits.md)
- [Stage 6 — Equivalence-checked coherence](predicate_typing/equivalence-checking.md)
- [Stage 7 — Syntax tuning and witness types](predicate_typing/syntax-tuning.md)

### Libraries — [libraries/](libraries/)

Give Koan a multi-file source surface, an in-language effect/error story, and
a canonical body of Koan code that exercises both. Each item is a piece of
substrate the standard library needs to exist as Koan source rather than as
Rust builtins:

- [Generalize `Scope::out` into monadic side-effect capture](libraries/monadic-side-effects.md)
- [Standard library](libraries/standard-library.md)

### Operator chaining — [operator_chaining/](operator_chaining/)

User-declarable operators and the n-ary chaining mechanism that evaluates them: a
recognized run of operators reduces by its group's declared mode — unary, fold, or
pairwise — and a module-scoped `GROUP`/`OP` surface populates the per-scope operator
registry the reducer walks.

- [User-defined operator modules](operator_chaining/user-defined-operator-modules.md)

### Type language — [type_language/](type_language/)

Engine-level type-language substrate — how modules, signatures, functors,
deferred-return FNs, record-shaped parameter binding, and VAL-slot identity
are represented in `KType` and routed through dispatch. The substrate the
predicate-typing stages and the stdlib's functor-heavy collections both
build on:

- [Constructors as first-class function values](type_language/constructor-as-first-class-function.md)
- [Anonymous structural unions](type_language/anonymous-unions.md)
- [Tagged-union variants as dispatchable types](type_language/tagged-variant-types.md)
- [SIG abstract vs manifest type members](type_language/sig-abstract-vs-manifest-types.md)

### Editor tooling — [editor_tooling/](editor_tooling/)

Surface that lets external tools — editors, debuggers, build systems — see
intermediate Koan state. The build-time / run-time scheduler split is the
foundation:

- [Two-phase execution: build-time with pegged inputs, run-time resume](editor_tooling/two-phase-execution.md)
- [Continue-on-error for the REPL and batch mode](editor_tooling/continue-on-error.md)

### Refactor — [refactor/](refactor/)

Cross-cutting cleanups that keep the engine legible and fast as it grows —
reconciling names with behavior, merging responsibilities that have drifted apart,
shrinking the unsafe surface, and cutting hot-path overhead:

- [Codebase-wide naming and responsibility audit](refactor/naming-and-responsibility-audit.md)
- [Unify the type-resolution-outcome enums](refactor/unify-resolution-outcome.md) —
  collapse `ElabResult` / `ResolveTypeExprOutcome` / `TypeLeafCarrier` into one generic
  `ResolveOutcome<T>` with a `map_done` lift.
- [Merge the raw-type-part slot markers](refactor/merge-raw-type-part-slots.md) —
  collapse the slot-only `KType::SigiledTypeExpr` / `RecordType` markers into one
  `RawTypePart(TypePartKind)`; `KExpression` stays distinct.
- [Content-addressed type identity](refactor/type-identity-registry.md) — replace
  `Rc::ptr_eq` nominal-type identity with a wide content-hash digest (`Copy`), with collision
  detection and a deferred cross-epoch repair path so thread-local digests merge lock-free.
- [Memoized subtype matching](refactor/memoized-subtype-matching.md) — cache dispatch
  admissibility outcomes per type, keyed by the candidate supertype's digest, so a repeat
  subtype check is an O(1) lookup instead of a structural walk.
- [Node-lifetime lift and contract re-anchor](refactor/node-lifetime-lift-and-contract.md) —
  thread distinct input/output node lifetimes through the lift and contract Done-boundary hooks so
  their re-anchor is node-to-node, retiring the `'run` fabrication `read_lifted` / `pin_carried_to_run` do.
