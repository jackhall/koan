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
  to a `KType::Record` that a third `FN` overload's signature slot
  admits. It makes the [standard library](libraries/standard-library.md)'s
  higher-order combinators ergonomic to call with an inline function. See
  [design/functional-programming.md § Anonymous functions](../design/functional-programming.md#anonymous-functions).
- *Duplication consolidation.* Six pre-located copy-paste clusters each collapsed to a
  single owner: per-builtin typed-binder `binder_name` (one shared
  [`type_part_binder_name`](../src/builtins.rs)), the FN and FUNCTOR bodies (one shared
  [`build_fn_like`](../src/builtins/fn_def.rs) keyed on `FnKind`), the `finish.rs` dep-finish/catch handler arms (one shared
  [`run_step`](../src/machine/execute/run_loop.rs)), the `dict_literal`
  `accept_colon`/`accept_equals` pair (one `accept_separator`), the slot-extract error
  envelope (one [`require_kexpression`](../src/machine/core/kfunction/action.rs)
  owning the parenthesized-slot error text), and the scheduler `Object`/`Type` finalize
  arms (one [`check_declared_return`](../src/machine/execute/finalize.rs)
  parameterized over the lifted carrier's `matches_value`/`matches_type` predicate).
- *Region unsafe consolidation.* Every captured/defining-scope re-attach is funnelled behind two
  audited [`scope_ptr`](../src/machine/core/scope_ptr.rs) handles, and the cycle-gate redirect
  recovers its target *from the value being stored* (the self-anchoring closure's captured scope
  names its defining region, so the redirect region is reached by walking that scope's `outer` chain)
  — so `Region` stores no escape owner, no allocation back-edge can form, and
  [`region.rs`](../src/witnessed/region.rs) carries no `unsafe`.
  The store-side erasure lives behind one `Stored` trait: all region-stored
  families route the scheduler's single audited `erase_to_static` (the safe direction of the
  one `retype` primitive the read-side re-anchor shares) and one gated `alloc` engine,
  replacing the per-type `T<'a> → T<'static>` transmute pairs with one. The scope-pointer surface
  split on whether the carrier can brand the scope's `'a`: the safe `BoundedScopePtr<'a>` makes
  `Module::child_scope`, `Signature::decl_scope`, `KFunction::captured` and a `Scope`'s `outer`
  reader-bounded re-hands carrying no `unsafe`. The two lifetime-free carriers each store a
  `&'static Scope` erased once through the safe `erase_to_static::<ScopeRefFamily>`, so the handle
  itself holds no `unsafe`: `CallFrame`'s per-call child scope rides the substrate's externally-witnessed
  `SealedExtern<ScopeRefFamily>` carrier (read back through the witness-bounded `SealedExtern::attach`),
  and a node's `NodeScope::YokedChild` rides an `ErasedScopePtr` (read back through the witness-bounded
  `ErasedScopePtr::reattach_witnessed`) — both fully-safe accessors taking the pinning `Rc` as an
  explicit `Witness`, the only routed `unsafe` being the shared `retype` in `witnessed.rs`. The
  per-call child's construction-time lifetime erasure (the region `pin_deref` and outer-link re-attach
  in `CallFrame::new` / `try_reset_for_tail`) is removed: `Scope::child_for_frame` builds the child at
  real lifetimes, brand-shortening the longer-lived lexical parent and run-global root to the fresh
  per-call region's lifetime, so the per-call child touches no `unsafe` at all. The
  `unsafe impl Reattachable` obligation is discharged once through a shared `reattachable!` macro
  instead of per-carrier.
  Honest slot storage landed for per-call frame scopes: a frame scope rides its slot as a
  payload-less [`NodeScope::Yoked`](../src/machine/execute/nodes.rs) marker re-projected from the
  slot's own `Node.frame` cart — no fabricated run-length `&'a` persists across a TCO reset.
  The frame re-anchor then landed in full: the free `&'run` scope fabrication at the read
  boundary is deleted, a within-step frame lifetime `'step` (`'a: 'step`) threads
  `run_dispatch`/`SchedulerView`/`BuiltinFn`, and a slot's scope is now read on demand via
  [`Scheduler::current_scope`](../src/machine/execute/run_loop.rs) through the witness-bounded
  [`CallFrame::scope_bounded`](../src/machine/core/arena.rs) brand (the post-step loop reads it
  through a `PostStep` token off the slot's returned frame).
  [`CallFrame::with_frame_interior`](../src/machine/core/arena.rs)'s seed bind no longer re-exposes
  the region at a free `'a`: it reaches the region through the child scope's own `region` field
  (a `Copy` `&'a KoanRegion`), so the seed side carries no free re-exposure. The full Miri-slate
  confirmation of the removed self-reference tokens has landed — the slate is green. And
  `KFunction::captured` now rides a `BoundedScopePtr`
  (see [design/per-call-region/scope-handles.md § Slot-table scope handle](../design/per-call-region/scope-handles.md#slot-table-scope-handle)).
  And [`KExpression`](../src/machine/model/ast.rs) joined the layout-invariant carrier families (its
  lifetime borne only by a `Spliced(Carried)` part), so `QUOTE` now yokes its owned splice-free
  expression via [`KoanRegion::alloc_witnessed_embedding`](../src/machine/core/arena.rs) — the
  object's region co-located by the `for<'b>` brand — rather than asserting it via `Witnessed::new`.
  See [design/memory-model.md § Region lifetime erasure](../design/memory-model.md#region-lifetime-erasure).
- *Shell-over-storage frame reuse.* `CallFrame` is now a thin shell over a refcounted
  [`FrameStorage`](../src/machine/core/arena.rs) (the per-call `KoanRegion` plus the ancestor
  `outer` chain). An escaping value (a returned closure, a functor-built module) pins only the
  storage, leaving the shell uniquely owned so `try_reset_for_tail` reuses it across a tail
  iteration instead of being foreclosed — only a live shell clone refuses. The four escape
  shapes and the cycle-gate walkers carry `Rc<FrameStorage>`; cross-reset region capture is
  borrow-checker-enforced for safe code (no new unsafe). See
  [design/per-call-region/frames.md § TCO frame reuse](../design/per-call-region/frames.md#tco-frame-reuse).
- *Per-node output lift.* A node continuation's output is bound to the per-step frame
  lifetime `'step` ([`Outcome<'step>`](../src/machine/execute/outcome.rs)), not `'run`. The
  producer keeps its terminal in its own frame (the slot's `Done` co-stores the backing
  `Rc<CallFrame>`, so frame death moves Done→free) and does not lift; each consumer
  pull-lifts its deps into its own region at read ([`run_step`](../src/machine/execute/run_loop.rs))
  through the single [`NodeLift`](../src/machine/execute/lift.rs) workload hook, so an
  intermediate value dies with its consumer and only a consumer-less root drains to the run
  region. Return-contract enforcement stays a separate Done-time layer. This output-lifetime
  shrink and lift hook were the prerequisite half of confining `'run` to `KoanRuntime`; the
  *workload-independent DAG runtime* that completes it has shipped — the scheduler is a
  crate-root [`mod scheduler`](../src/scheduler.rs) generic over a `Workload`, naming no Koan
  value, scope, memory, or AST type, with the Koan driver in
  [`execute::run_loop`](../src/machine/execute/run_loop.rs). The value-movement re-anchors on this
  path are now node-scale, not `'run`: `NodeFinalize::finalize_terminal` is single-lifetime
  (`'o -> 'o`), a `Done` terminal is finalized at its step lifetime `'step` *within* the producing step
  (`NodeStep<'step>`; `run_step` owns the step start to finish — enter, run, finalize — over the cart clone that witnesses `'step`),
  and the consumer-pull / `Outcome::Forward` lift re-anchors through [`read_lifted`](../src/machine/execute/runtime.rs)
  into the consumer scope region, with the run-global root drain re-anchoring through `lift` itself.
  The dispatch decide surface then collapsed onto that single cart-scale lifetime:
  `NodeScope::Anchored` is gone (every slot scope is cart-witnessed), `Outcome` is single-lifetime
  (the `Outcome<'run, 's>` split retired along with the `shorten_outcome` / `deps_for_builtin` /
  `obj_for_builtin` up/down bridges), and the `run_step` continuation reattach targets the
  step lifetime the held cart `Rc` witnesses — leaving `deps_at_step` (now a safe
  `reattach_slice_with`) the only `outcome.rs` re-anchor. The dispatch decide functions then dropped their conflated
  `'run` for a single cart lifetime `'step`: a decide reads scope and produces `Outcome` at `'step`,
  the picked `KFunction` is cart-scale (read from the `'step` scope), and the pristine-AST lifetime
  `'ast` (`'ast: 'step`) is named only at the submission boundary
  ([`submit_dispatch`](../src/machine/execute/dispatch/submit.rs)), where a borrowed
  `&KExpression<'ast>` is read against the cart scope — the working expression is re-anchored from
  its erased node carrier to `'step`, so decide never holds a live `'ast` borrow. See
  [design/per-call-region/lifecycle.md § Consumer-pull node-output lift](../design/per-call-region/lifecycle.md#consumer-pull-node-output-lift).
- *Unified erase/reattach carriers.* The hand-rolled erase-to-`'static` /
  reattach carriers (the scope pointers, the contract, the continuation, the scheduler's `Erased<W::Value>`)
  and the cluster of one-off `outcome.rs` reference reattaches now share one generic
  [`Erased<T>`](../src/witnessed.rs) owner over an `unsafe trait Reattachable { type
  At<'r>; }` lifetime-family. A single `retype` primitive (a `ManuallyDrop` `transmute_copy`) is the
  only lifetime-retype site; each carrier family is declared beside its own type
  (`ContractFamily`, `CarriedFamily`, `ContinuationFamily`, `KObjectFamily`, `ScopeFamily`, …) through
  the shared `reattachable!` macro, which discharges the layout-invariance `unsafe impl` obligation
  once, with no `transmute` of its own. The scheduler then took sole ownership of all three inter-node
  carrier reattaches: the continuation and contract — like the value channel before them — are
  stored `Erased` on the lifetime-free node and re-anchored only through
  [`vend_carrier`](../src/witnessed.rs), one safe-signature wrapper whose returned `'w` the
  compiler bounds against a witness borrow `&Rc<W::Frame>` the driver passes, so the `run_loop.rs` /
  `finalize.rs` call sites carry no `unsafe` of their own. See
  [design/memory-model.md § Region lifetime erasure](../design/memory-model.md#region-lifetime-erasure).
- *Witnessed value carrier.* The scattered `Reattachable` / `Erased` / `retype` machinery now lives
  in the top-level [`witnessed`](../src/witnessed.rs) module (a sibling of `machine` / `scheduler`),
  and a node's value slot stores a single [`Sealed<Carried, FrameSet>`](../src/witnessed.rs)
  bundling the erased value with the producer-frame witness set that pins it — the witness-pins-the-value
  relationship is a type invariant, not a co-stored pair. Reads go through the safe `with` / `map` /
  `read` accessors (the first two rank-2 `for<'b>` branded against escape, all three `compile_fail`-
  and Miri-tested), retiring the open-coded `read_result` reattaches and `pin_carried_to_run`. The
  consumer-pull lift is now a borrow-checked relocation (`relocate_carried`, a safe `deep_clone` +
  `alloc` at the step brand, wrapped as the `Sealed::transfer_into` `merge`), so **no value-path
  `unsafe` reattach remains**. The co-location-enforcing
  constructor `yoke` (sourcing a carrier from the witness's own region behind a `for<'b>` brand, so
  the witness-pins-the-value invariant holds by construction) and the `merge` composition law
  (combining two carriers under one brand, re-sealed under the union of their witness sets, with
  `outer`-chain subsumption dropping a member another already pins) have also landed. The keystone
  run-loop restructure and its consuming `open`, the production witness impls, the unified `FrameSet`
  set-witness (result slot and scope handle on one region-owner type), and the per-value frame
  anchor's removal — a stored value now holds no owning `Rc` back to a region, so the allocation
  engine needs no cycle gate, and an escaping closure / module is kept alive by its carrier's witness
  set while it rides a slot and retained onto the consumer frame when relocated out; this closed the
  lift-relocation `unsafe` and cleared the process-exit leak — have since landed too. The
  `alloc`-side carrier adoption has begun: the consumer-pull lift now hands each construction finish
  its deps as their producer slots' own `Sealed` carriers (`Sealed::duplicate` / `Scheduler::dep_carrier`,
  the `DepTerminal` carrier), and the object family's region-pure leaves and aggregates are born
  witnessed by `yoke` / `transfer_into` — a single-part literal, a static aggregate cell, and the
  dep-carrier-fed list / dict / record fold, no longer paired with an asserted `Witnessed::new`. The
  carrier-self-building object constructions then joined them: the newtype / tagged-union constructors
  and `catch` fold their dep carriers via `transfer_into` / `merge` (the nominal type identity crossing
  the build brand as a non-object `RegionTypeFamily` operand), and FN def `yoke`s its co-located
  `KObject::KFunction` onto a carrier witnessed by the defining scope's frame. The
  remaining value-embedding object sites (the bare-arg `attr` / `FROM` / literal Resolved arm) and the
  type family are tracked by the [per-node-memory](per-node-memory/) project. See
  [design/memory-model.md § Region lifetime erasure](../design/memory-model.md#region-lifetime-erasure).
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
  [design/typing/ktype/README.md](../design/typing/ktype/README.md).
- *One bare-leaf type resolver.* The synchronous `coerce_type_token_value` is folded into
  [`resolve_type_leaf_carrier`](../src/machine/execute/dispatch/resolve_type_identifier.rs) over the
  memoized, park-capable `Scope::resolve_type_identifier` bridge, so a bare type-name leaf resolves
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
  `RecursiveRef`), consumed by `Scope::resolve_type_identifier`. Modules and signatures travel in
  the `Type` arm (projected by `require_module` / `require_signature`), and a shallow
  [`KKind` `{ Proper, Module, Signature, Any }`](../src/machine/model/types/kkind.rs) carried
  as `KType::OfKind` classifies a type at dispatch by its kind. See
  [design/typing/elaboration.md](../design/typing/elaboration.md) and
  [design/execution/README.md](../design/execution/README.md).
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
  [design/typing/ktype/README.md](../design/typing/ktype/README.md) and
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
  See [design/execution/scheduler.md § The dispatcher / scheduler boundary](../design/execution/scheduler.md#the-dispatcher--scheduler-boundary).
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
  [design/execution/scheduler.md § The dispatcher / scheduler boundary](../design/execution/scheduler.md#the-dispatcher--scheduler-boundary).
- *`NodeWork` carries no AST.* The scheduler's slot-work collapsed to a single
  [`NodeWork`](../src/machine/execute/nodes.rs) struct — one captured `SchedulerView -> Outcome`
  `cont` (the combine/catch/decide behavior built in by combinators; the `<deps>` dep-error label
  is harness policy) plus a pre-rendered deadlock-summary string. Binder-install and the recursive eager-sub pre-submission moved out of `Scheduler::submit_node`
  into a dispatch-layer [`submit_dispatch`](../src/machine/execute/dispatch/submit.rs) chokepoint,
  so the scheduler never introspects a `KExpression` — `submit_node` is a generic slot allocator
  and no `NodeWork` variant names an AST. See
  [design/execution/classify-and-apply.md § Dispatch birth and resume](../design/execution/classify-and-apply.md#dispatch-birth-and-resume).
- *Literal lowering hoisted to the dispatcher.* The aggregate-literal lowering
  (`schedule_list_literal` / `schedule_dict_literal` / `schedule_record_literal`, the `Slot`
  layout enum, `classify_aggregate_part`, `resolve_aggregate_bare_name`) moved from
  `scheduler/literal.rs` to [`dispatch/literal.rs`](../src/machine/execute/dispatch/literal.rs),
  next to the harness that drives it. It was the one file in the scheduler subtree that
  name-resolved and built values from an `ExpressionPart`, so the "scheduler names no AST"
  invariant now holds structurally across `scheduler/**`. The methods are `&mut self` on
  `KoanRuntime` (below), reached through the scheduler's public surface (`submit_in_own_scope` /
  `current_scope`). See [design/execution/README.md](../design/execution/scheduler.md#the-dispatcher--scheduler-boundary).
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
- [`alloc_ktype` returns `Witnessed`](per-node-memory/alloc-ktype-witnessed.md)
- [Module system stage 5 — Modular implicits](predicate_typing/modular-implicits.md)
- [Move binder discovery into the parser](refactor/binder-discovery-to-parse.md)
- [Enforce the type/value split in Bindings](refactor/enforce-bindings-type-value-split.md)
- [Fold `Dep` into `DepRequest`](refactor/fold-dep-into-deprequest.md)
- [Collapse the machine model/core straddle](refactor/machine-straddle-colocation.md)
- [Memoized subtype matching](refactor/memoized-subtype-matching.md)
- [Merge the raw-type-part slot markers](refactor/merge-raw-type-part-slots.md)
- [Codebase-wide naming and responsibility audit](refactor/naming-and-responsibility-audit.md)
- [Region-store records and resolved KTypes](refactor/region-store-records-and-ktypes.md)
- [Structural value equality](refactor/structural-value-equality.md)
- [Content-addressed type identity](refactor/type-identity-registry.md)
- [Unify the two argument binders](refactor/unify-argument-binders.md)
- [Unify the value-name lookup outcomes](refactor/unify-name-lookup-outcome.md)
- [Unify the type-name resolution path](refactor/unify-resolution-outcome.md)
- [Constructing circular values](type_language/circular-value-construction.md)
- [Constructors as first-class function values](type_language/constructor-as-first-class-function.md)
- [Function-typed return annotations](type_language/function-typed-return-annotations.md)
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
- [Constructing circular values](type_language/circular-value-construction.md)
- [Function-typed return annotations](type_language/function-typed-return-annotations.md)

### Editor tooling — [editor_tooling/](editor_tooling/)

Surface that lets external tools — editors, debuggers, build systems — see
intermediate Koan state. The build-time / run-time scheduler split is the
foundation:

- [Two-phase execution: build-time with pegged inputs, run-time resume](editor_tooling/two-phase-execution.md)
- [Continue-on-error for the REPL and batch mode](editor_tooling/continue-on-error.md)

### Per-node memory — [per-node-memory/](per-node-memory/)

Grow the shipped `witnessed` carrier into a generic, Koan-free substrate for per-node
scheduler memory — a sealed node-storage form, its access verbs, and the generic bump
allocator — then migrate the engine's value, scope, continuation, and contract carriers
onto it. The construction primitives (`yoke` / `merge` / `with` / `map`, the
witness-borrow reattaches), the generic `Region<P>` bump allocator beside its carrier in
the `witnessed` module, the opaque [`Sealed`](../src/witnessed.rs) storage form (read
through a rank-2 `open`, result slot rerouted onto it), the run-loop step restructure and
its consuming `open`, the region-pure / aggregate construction inversions, the
carrier-self-building object constructions (the newtype / tagged-union constructors, `catch`, and FN
def now build witnessed via `transfer_into` / `merge` / `yoke`, with the nominal type identity crossing
the build brand as a non-object `RegionTypeFamily` operand), the
per-scope sealed reach-set (a `FrameSet` on `Scope` that folds a deposited value's reach,
omits the home frame and its lexical ancestors, and seals at scope close — `close` wired finalize-time
and owner-routed for per-call frames, `MODULE` / `SIG`, and the run root), and the
**carrier-delivered object embeds** (the bare-arg value-embedding sites — `attr`, `FROM`, the literal
Resolved arm — `merge` a delivered `Sealed` carrier, and `let` / user-fn arg binds fold the bound
value's full carrier into the reach-set, taking the whole object channel off the single-frame
`reached_frame` reconstruction) are all
shipped. The design is captured in
[design/per-node-memory.md](../design/per-node-memory.md). What remains migrates as one
linear chain: the type family converts onto `yoke`, taking the last `KType::Module` user off the
single-frame `reached_frame` / `FrameStorage.retained` reconstruction so it can be deleted — then the
consumption reads converge on a single `open` verb:

- [`alloc_ktype` returns `Witnessed`](per-node-memory/alloc-ktype-witnessed.md) — convert the type
  family onto `yoke`; the last `KType::Module` user converted, `reached_frame` and the per-frame
  `retained` field are deleted and the step `pin` becomes exact.
- [Migrate the consumption reads onto `open`](per-node-memory/reads-to-open.md) — restructure the
  result-slot value reads, scope-handle reads, and ~40 loose `reattach_*` sites onto `open` + copy-out /
  CPS, deleting the transitional self-witnessed `read` and both wrappers.
- [`Sealed`: a single access verb](per-node-memory/single-open-verb.md) — delete the transitional
  `attach` and the externally-witnessed read path, leaving `Sealed` with `open` alone.

### Refactor — [refactor/](refactor/)

Cross-cutting cleanups that keep the engine legible and fast as it grows —
reconciling names with behavior, merging responsibilities that have drifted apart,
shrinking the unsafe surface, and cutting hot-path overhead:

- [Codebase-wide naming and responsibility audit](refactor/naming-and-responsibility-audit.md)
- [Collapse the machine model/core straddle](refactor/machine-straddle-colocation.md) —
  co-locate the value↔scope↔closure strongly-connected component that straddles the
  model/core boundary into one module, turning its cross-boundary cycle edges (the
  `α·feedback` charge) into free intra-module edges.
- [Unify the type-name resolution path](refactor/unify-resolution-outcome.md) —
  collapse `ElabResult` / `TypeIdentifierResolution` / `TypeLeafCarrier` into one generic
  `ResolveOutcome<T>` with a `map_done` lift, and stop repeating the `from_name`
  builtin-table fallback across the `from_type_identifier` / `elaborate` resolver layers.
- [Merge the raw-type-part slot markers](refactor/merge-raw-type-part-slots.md) —
  collapse the slot-only `KType::SigiledTypeExpr` / `RecordType` markers into one
  `RawTypePart(TypePartKind)`; `KExpression` stays distinct.
- [Content-addressed type identity](refactor/type-identity-registry.md) — replace
  `Rc::ptr_eq` nominal-type identity with a wide content-hash digest (`Copy`), with collision
  detection and a deferred cross-epoch repair path so thread-local digests merge lock-free.
- [Memoized subtype matching](refactor/memoized-subtype-matching.md) — cache dispatch
  admissibility outcomes per type, keyed by the candidate supertype's digest, so a repeat
  subtype check is an O(1) lookup instead of a structural walk.
- [Unify the two argument binders](refactor/unify-argument-binders.md) — stop the builtin
  dispatch path building a whole `KFuture` just to gut `future.args`; one arg-binding path
  instead of `bind` (`Record<ArgValue>`) beside `bind_by_name` (`Record<Carried>`).
- [Unify the value-name lookup outcomes](refactor/unify-name-lookup-outcome.md) — name the
  bound/parked/unbound disposition shared by core `Resolution` and execute `NameOutcome` once,
  without minting a third `ResolveOutcome`.
- [Fold `Dep` into `DepRequest`](refactor/fold-dep-into-deprequest.md) — the two dep enums
  carry an identical `Dispatch`/`Existing` core (and already share `DepPlacement`); give them a
  shared core or a visibly-related pair.
- [Move binder discovery into the parser](refactor/binder-discovery-to-parse.md) — verify the
  AST recursion that finds a submission's binders, then cache its parse-static portion on
  `KExpression` (beside `DispatchShape`) instead of re-deriving it at every submission.
- [Enforce the type/value split in Bindings](refactor/enforce-bindings-type-value-split.md) —
  the committed `types`/`data` maps are partitioned, but the split is held by per-callsite
  convention and the in-flight `placeholders` map carries no type/value discriminant; make the
  distinction structural in the `Bindings` API.
- [Region-store records and resolved KTypes](refactor/region-store-records-and-ktypes.md) — hold
  a record's `Box<Record<KType>>` field-type memo and an already-region-allocated `KType` by
  region reference, killing the `alloc_ktype(kt.clone())` and `.ktype()` deep clones on the
  resolve/bind/lift paths.
- [Structural value equality](refactor/structural-value-equality.md) — replace the
  `summarize() == summarize()` string comparison (`Parseable::equal`, and the dict-key
  `Hash`/`Eq`) with a per-variant structural compare that gets NaN, nominal identity, record
  field order, and type parameters right.
