# Classify and apply

The shape classifier, the no-keyword fast lanes, the keyworded apply-a-callable
pipeline, and the birth/resume decide closures — the execute half of the dispatch
pipeline (the submit half is
[Name placeholders and submission](name-placeholders.md)). Part of the
[execution model](README.md).

The execute side — [`classify_dispatch`](../../src/machine/execute/dispatch.rs) —
opens with a pre-walk shape classifier. `classify_dispatch_shape` sweeps the
expression's parts for any `Keyword` first and, if none, branches on the head
token's shape, producing a `DispatchShape` variant. The no-keyword fast-lane
variants run their own handlers and never enter
`Scope::resolve_dispatch_with_chain`: there are no candidates in
`bindings.functions` for these shapes, so the candidate machinery would do no
useful work. The single-part lanes (`BareIdentifier`, `BareTypeLeaf`,
`SigiledTypeExpr`, `RecordType`, `LiteralPassThrough`) surface a name or value directly, while
the multi-part head-position call lanes (`TypeCall`, `FunctionValueCall`,
`HeadDeferred`, `TypeHeadDeferred`) each resolve their head to a callable and
converge on the [shared apply-a-callable tail](name-placeholders.md#dispatch-time-name-placeholders). A
non-callable multi-part head is `NonCallableHead`, a direct `DispatchFailed`
from the dispatch entry. The `Keyworded` variant — produced only when a real
keyword is present — falls into the chain-walked resolution plus eager
name-resolve plus dep-schedule pipeline below.

The keyworded pipeline runs in four steps. Step 1 builds the bare-name
outcome cache: one
[`resolve_name_part`](../../src/machine/execute/dispatch.rs) call per
bare-name part of `expr` (`Identifier` or leaf `Type`) into
`bare_outcomes: Vec<Option<NameOutcome<'a>>>`, with `None` for non-bare-name
parts. The cache is built with `consumer = None` so cycle detection is
deferred to Step 4, where it runs only on slots the picked function
classifies as references (a binder declaration slot like `x` in `LET x = …`
has the dispatching slot as its own placeholder's producer, so an upfront
cycle check would false-positive on declarations). Step 2 sweeps the cache
for `NameOutcome::ProducerErrored`: a bare-name arg whose producer
terminalized with an error can never resolve, so it propagates upfront with
a `<wrap-resolve>` frame before any candidate work.

Step 3 calls
[`Scope::resolve_dispatch_with_chain`](../../src/machine/core/scope.rs) once,
passing the cache as `bare_outcomes: &[Option<NameOutcome<'a>>]`. Admission
is strict-only: [`signature_admits_strict`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
reads each bare-name slot's cached outcome rather than re-resolving it per
scope. A `Resolved(obj)` cache entry admits iff
[`KType::accepts_part`](../../src/machine/model/types/ktype_predicates.rs)
holds for the carried type — a bare name whose value has the wrong carrier
type strict-rejects the overload, and the call surfaces as `DispatchFailed`
rather than a bind-time `TypeMismatch`. `Parked` / `Unbound` cache entries
admit via shape-only `arg.matches(part)`: the post-pick splice/park walk in
Step 4 is the only place that produces precise per-slot `ParkOnProducers` /
`UnboundName` diagnostics, so admission must not reject and lose them. The
match on [`ResolveOutcome`](../../src/machine/core/scope.rs) is:
`Resolved(r)` continues into Step 4 with the strict-picked function plus
the per-slot index buckets `r.slots` carries (`wrap_indices`,
`ref_name_indices`, `eager_indices`); `Ambiguous(n)` surfaces as an
`AmbiguousDispatch` error; `Unmatched` surfaces as `DispatchFailed`;
`Deferred` (the candidate may match after sub-evaluation yields a typed
`Future(_)`) routes to `KeywordedState::install_eager_only`, which declares every
eager-shaped part as a dep-finish dependency and parks this slot on them;
the splice finish re-resolves dispatch against the spliced expression at
dep completion;
`ParkOnProducers(_)` and `UnboundName(_)` are decided inside the scope walk
as described below.

`resolve_dispatch_with_chain` decides each visible scope's contribution as
it walks innermost-first, from the finalized overloads and the visible
in-flight pending producer the scope's `FunctionLookup` surfaces together.
The innermost scope to reach a terminal outcome wins; only `UnboundName` and
`Unmatched` are decided post-walk. Per scope:

1. A visible pending sibling parks the scope (`ParkOnProducers`) — it would
   shadow any finalized overload here once it finalizes, so the scope
   resolves nothing until it does, even over a same-scope finalized
   strict-Pick. The wake re-dispatches against the now-registered overload.
2. Otherwise the strict gate Picks the most-specific admitting overload
   (`Resolved`), surfaces a genuine tie (`Ambiguous`), or — on a tie with an
   unevaluated eager part that may break it — `Deferred`.
3. Otherwise (strict-Empty) one relaxed-admission pass per candidate assumes
   every unresolved slot satisfiable and resolves by what each leaned on: a
   `Parked` bare name (a producer exists) ⇒ `ParkOnProducers`; otherwise an
   unevaluated eager part ⇒ `Deferred`; otherwise a `Dead` unbound bare name
   records an `UnboundName` blocker without terminating — an unbound name
   never arrives, so it never parks, and holding it back lets an outer scope
   still strict-Pick the bare name shape-only as an `:Identifier` / `:Any`
   slot.

After the walk: a recorded dead-lean blocker ⇒ `UnboundName(name)`; nothing
contributed even a dead lean ⇒ `Unmatched`. Parked outranks eager (a parked
bare name is just an eager part whose value arrives from a producer), and
eager outranks the dead-lean `UnboundName` because an eager part's evaluation
may itself surface the precise diagnostic — surfacing `UnboundName` first
would pre-empt an Expression-in-Type-slot dispatch (`(maybe) some 42`) whose
head evaluates to the schema after one sub-Dispatch.

The rails the dispatch driver feeds:

- **Fast lane** (pre-walk classifier, runs before any resolve walk).
  `classify_dispatch_shape` is one pass over `expr.parts`: keyword anywhere
  ⇒ `Keyworded` (refined to `OperatorChain` for the chain shape); single-part
  `Identifier` ⇒ `BareIdentifier`; single-part leaf `Type` ⇒ `BareTypeLeaf`;
  single-part `SigiledTypeExpr` ⇒ `SigiledTypeExpr`; single-part `:{…}`
  `RecordType` ⇒ `RecordType`; single-part literal /
  value ⇒ `LiteralPassThrough`. With the no-keyword precondition established,
  a multi-part expression branches on its head: leaf-`Type` head ⇒ `TypeCall`;
  `Identifier` head ⇒ `FunctionValueCall`; nested-`Expression` head ⇒
  `HeadDeferred`; `:(...)` `SigiledTypeExpr` head ⇒ `TypeHeadDeferred`; a
  literal / list / dict / record-literal / record-type head ⇒ `NonCallableHead`
  (a record *type* is a value, not a callable). The "sweep first,
  branch on head second" ordering matters: a mixed shape like `(f IF x)`
  goes to `Keyworded` because only the candidate machinery knows how to
  dispatch the `(_ IF _)` bucket. `Keyworded` is never a catch-all for an
  unclassified head — a non-callable head is its own `NonCallableHead` sink.

  Each fast-lane variant has its own handler:

  - `BareIdentifier` (`(some_var)`) — `single_poll::bare_identifier` consults
    `Scope::resolve_with_chain` against the consumer's `LexicalFrame`:
    `Value` returns a `Done` outcome inline, `Placeholder` returns
    `Outcome::Forward(producer)`, whose harness splices the slot out as an alias
    of that producer (see [Bare-name forward splice](scheduler.md#bare-name-forward-splice)),
    `UnboundName` falls through to the keyworded path so `value_lookup`'s body
    produces the structured error.
  - `BareTypeLeaf` (`(Number)`, `(IntOrd)`) — `bare_type_leaf`
    routes through `resolve_type_leaf_carrier` over the memoized,
    park-capable `Scope::resolve_type_identifier` bridge: a leaf naming an
    earlier still-finalizing binder parks on its producer and re-resolves
    on wake, like every compound type form, and other failures surface
    directly. There is no candidate-machinery alternative for a bare leaf
    type. See
    [typing/elaboration.md § Layers](../typing/elaboration.md#layers)
    § Layer 4 for the shared resolver seam.
  - `TypeCall` (`MyStruct {x = 1}`, `MyFunctor {T = IntOrd}`) —
    [`type_call`](../../src/machine/execute/dispatch/single_poll.rs)
    resolves the head Type token to its `bindings.types` identity. A
    `SetRef` identity is a `ResolvedCallable::Constructor`; a
    `KType::KFunctor { body: Some }` (a bound functor in the type table) is a
    `ResolvedCallable::Function`. Both flow through the shared
    apply-a-callable tail (below). No value-side carrier is fetched — the
    schema rides the identity. Opaque / Module / unbound heads surface a
    `TypeMismatch`. A head token bound to a still-finalizing producer (a
    forward functor `LET`) parks on it and re-runs `type_call` on resume.
  - `SigiledTypeExpr` (single-part `:(...)` wrapper) — the `classify_dispatch`
    arm tail-replaces the slot with a fresh `Decide`
    of the wrapped `KExpression`, so the inner expression runs through the
    same classifier and produces the same carrier shape any other dispatch
    site does. See
    [type-language-via-dispatch.md](../typing/type-language-via-dispatch.md)
    for the full type-language dispatch contract.
  - `RecordType` (single-part `:{…}` record type) — `record_type` folds the
    field list straight to `KType::Record` through the shared field-list
    elaborator (no tail-replace, no internal type-constructor builtin),
    deferring through a dep-finish `cont` only when a field type forward-references
    or sub-dispatches. See
    [type-language-via-dispatch.md § Record-type sigil](../typing/type-language-via-dispatch.md#record-type-sigil).
  - `FunctionValueCall` (`f {x = 7}`) — [`FnValueState`](../../src/machine/execute/dispatch/fn_value.rs)
    resolves the `Identifier` head and handles every admission outcome
    directly. The call shape admits iff `expr.parts[1..]` is exactly one
    nested-parens part (the *only* call shape — koan has no `f 1 2`
    positional call syntax for function values, so the named-arg shape
    is the whole user-facing surface). A `KFunction(f, _)` head resolves to a
    `ResolvedCallable::Function` and a `KType::SetRef { .. }` head in the value channel's
    `Type` arm — the identity a value-classified alias of a constructible type
    surfaces (`LET outcome = Outcome` then `(outcome (Err "x"))`) — to a
    `ResolvedCallable::Constructor`, both flowing through the shared
    apply-a-callable tail (below). Any other carrier (number, string, instance
    struct, module, …) surfaces a `TypeMismatch` directly. A `Placeholder` head
    installs the head-placeholder park; an unbound head surfaces
    `UnboundName(name)` directly — this shape never falls through to
    `Keyworded`. Reconstruction errors from
    `KFunction::reconstruct_positional` (missing / unknown /
    duplicate-named args, malformed pair shapes) surface as
    the `Err` arm of a node result with the same structured wording the keyworded
    path produces.
  - `HeadDeferred` (`(pick) {x = 1}`) and `TypeHeadDeferred`
    (`:(MyFunctor {base = IntOrd})`) — [`HeadDeferredState`](../../src/machine/execute/dispatch/head_deferred.rs)
    sub-dispatches the head first (an Owned edge; the park/resume pair mirrors
    `CtorState`'s), then branches the resumed value's kind into a
    `ResolvedCallable`. `HeadDeferred` admits a function, functor, bound functor,
    or constructible type; `TypeHeadDeferred` (the `:(...)` sigil guarantees a
    type) prunes the plain-function arm and surfaces a type-shaped `TypeMismatch`
    on a non-type. Both then run the shared apply-a-callable tail.

  **The shared apply-a-callable tail.** All four head-position call lanes —
  `TypeCall`, `FunctionValueCall`, `HeadDeferred`, `TypeHeadDeferred` —
  converge on [`apply_callable`](../../src/machine/execute/dispatch/apply_callable.rs).
  A `ResolvedCallable` has exactly two execution arms: `Constructor(&KType)`
  builds from a struct / tagged / newtype / `TypeConstructor` schema, and
  `Function(&KFunction)` calls a `KFunction` by name. A functor is a `KFunction`
  whose result is a module, so functor application is the `Function` arm — the
  functor/function distinction survives only at classification (for `KFunctor`
  typing and the `TypeHeadDeferred` diagnostic gate), never at execution. The
  tail body-shape-branches `expr.parts[1..]` (`extract_call_body` admits one
  `{name = value}` record literal or one `(value)` paren group) and launches
  construction or a `reconstruct_positional` + eager-subs function call. The
  eager-subs stage resolves the reconstructed call's bare-name value slots — the
  `wrap_indices` set from `classify_for_pick` — by sub-Dispatch, the same lane as
  `Expression` / literal args, so each resolves to its `Future` carrier before
  `KFunction::bind`. The committed callable's slot admission (`accepts_part`) then
  runs the carried-type check at bind: a `:Signature` slot consults the witness
  module's `compatible_sigs`, exactly as the keyword-led path does. Because the
  head has already selected the one callable, the keyword path's pre-pick
  `bare_outcomes` resolution (which exists to choose among co-bucket overloads) is
  unneeded here; a genuinely non-satisfying arg is a terminal `TypeMismatch`, not a
  fall-through, since there is no other overload to try.

  Forward references resolve through the fast lane and the eager
  name-resolve rail (below), both of which route name lookups through
  `Scope::resolve_with_chain` against the consumer's `LexicalFrame` and so
  consult the visibility-gated `placeholders` table. A *keyword-headed*
  call — `ID 7`, where `ID` is the head Keyword — dispatches through the
  `functions` bucket, which applies the same per-overload visibility filter
  (see [ktype/dispatch.md § Overload bucket visibility filter](../typing/ktype/dispatch.md#overload-bucket-visibility-filter)).
  A later-sibling overload registered after this consumer's statement is
  hidden, and dispatch falls through to outer scopes; finding nothing
  surfaces as `DispatchFailed`. Forward calls between sibling FNs work
  through the bucket-keyed `pending_overloads` channel: each sibling FN
  install appends a distinct entry to the per-bucket vec, and a parking
  consumer wakes on the earliest-index visible producer, re-parking on
  the next-earliest if its pick doesn't admit. Forward calls from a
  function *body* are unaffected because bodies re-dispatch per call
  against the body's lexical chain, by which point every sibling binder
  has registered.
- **Placeholder install** (Step 3.5). If the picked function carries a
  `binder_name` extractor, the driver installs `name → NodeId(idx)` into
  `placeholders` on the dispatching scope. If it carries a `binder_bucket`
  extractor, the driver appends a `(NodeId(idx), BindingIndex)` entry
  into `pending_overloads[bucket]` on the same scope. Each binder uses
  exactly one of the two channels — the `BinderKey` enum in
  [`submit.rs`](../../src/scheduler/alloc.rs) makes the
  dichotomy a type-level fact. Both installs are lenient against the
  matching submission-time install (see [Submission-time binder install
  and recursive sub-Dispatch](name-placeholders.md#submission-time-binder-install-and-recursive-sub-dispatch)
  below) — a `(name, idx)` pair already installed at submission re-applies
  cleanly here, and a bucket entry already appended at submission is not
  re-appended. A `Rebind` collision on the name channel against a
  different producer surfaces as a `Done(Err(_))` step so other slots
  keep draining; bucket-channel installs never Rebind (sibling appends
  are the intended shape).
- **Fused splice / park / eager-sub walk** (Step 4). One iteration over
  `expr.parts` co-handles the three per-slot rails the strict pick
  carries: wrap-slot splice (`resolved.slots.wrap_indices`), ref-name-slot
  park (`resolved.slots.ref_name_indices`), and eager sub-Dispatch
  scheduling (filtered by `resolved.slots.eager_indices` when the picked
  function is a lazy candidate, otherwise every eager-shaped part
  schedules). Per part, exactly one arm fires.

  Wrap and ref-name arms read the same `bare_outcomes[i]` cache the
  resolver consumed in Step 3 — so each bare name is resolved once per
  dispatch poll, shared across admission and the walk.
  Per-arm behavior:

  - **Wrap slot.** `Resolved(obj)` rewrites the slot to
    `ExpressionPart::Future(obj)` in place. `Parked(p)` cycle-checks
    via [`DepGraph::would_create_cycle`](../../src/scheduler/dep_graph.rs)
    and either surfaces `SchedulerDeadlock { sample: "cycle in type alias
    `<name>`" }` on a self-park or pushes `p` onto the shared
    `producers_to_wait` list. `Unbound(name)` surfaces a slot-terminal
    `UnboundName` (the parent binder's dep-finish reads it through
    `read_result(dep)` and short-circuits with the right framing — an
    `Err` from `execute` would break that catch).
    `Cycle` / `ProducerErrored` are unreachable here: the cache is built
    with `consumer = None`, and the Step 2 sweep already short-circuited
    `ProducerErrored`.
  - **Ref-name slot.** Literal-name slots keep the bare token, so
    `Resolved` and `Unbound` are no-ops. `Parked(p)` runs the same
    cycle-check then push as the wrap arm. Only `Identifier` and leaf
    `Type` parts park here; non-bare-name parts are skipped by
    classification.
  - **Eager-sub slot.** `Expression` parts sub-Dispatch; `SigiledTypeExpr`
    and `RecordType` parts wrap into a single-part `KExpression` and
    sub-Dispatch (the sub-Dispatch enters `classify_dispatch`'s matching shape arm —
    `SigiledTypeExpr` tail-replaces with the inner dispatch, `RecordType` folds
    to `KType::Record`); `ListLiteral` and `DictLiteral`
    route through `schedule_list_literal` / `schedule_dict_literal` for the
    aggregate dep-finish; any other shape rides through unchanged. Lazy
    `Expression` parts in `KExpression` slots are filtered out by
    `eager_indices` and the receiving builtin dispatches them itself.

  **Park-precedence guard.** Sub-Dispatch and aggregate scheduling are
  staged into a `PendingSub` vec rather than submitted eagerly during the
  walk. After the loop, if `producers_to_wait` is non-empty the decide
  returns a `ParkThenContinue` whose continuation is a `Continuation::Resume`
  (carrying a `ResumeFn` closure over the partly-spliced `working_expr`) — the
  harness installs the park edges as `Notify` (via `add_park_edge`) and
  installs a resume dispatch decide, so the captured
  `working_expr` becomes the source of truth on wake — **without** submitting
  any staged subs. Eager submission would leak the sub-nodes on the re-resume
  wake path, where the closure would re-stage them. Multi-name forward
  references compose as one combined park rather than N independent
  sub-Dispatches.

  If no producer parked, the driver applies each `PendingSub`: `Reuse(id)`
  for slots already pre-submitted recursively at outermost-submission time
  (see [Submission-time binder install and recursive
  sub-Dispatch](name-placeholders.md#submission-time-binder-install-and-recursive-sub-dispatch)),
  `Dispatch(sub_expr)` for a fresh sub-Dispatch, and `ListLit` / `DictLit`
  for the aggregate. With no subs to schedule the driver binds the picked
  function directly: the decide folds the resolved call into a dep-free
  `Outcome::Continue` (via `dispatch::exec::invoke_continue`) whose frame
  placement installs the per-call cart and whose `work` re-decides via
  `dispatch::exec::invoke` on the next pop
  (a wrap-slot-only call like `MAKESET IntOrd` resolves bare names in Step 4,
  leaves no eager parts, and binds in one step — no dep-finish detour). Otherwise
  the decide returns a `ParkThenContinue` with a `Continuation::Finish`
  declaring the fresh subs as deps with a splice finish; the harness parks the
  slot as a dep-finish carrying the finish. At dep completion the finish
  re-resolves the spliced `working_expr` and folds it into a `Continue` — via
  `invoke_continue` on the speculatively-picked function, or via
  `redispatch_continue` (re-running
  [`keyworded::finish`](../../src/machine/execute/dispatch/keyworded.rs)) when
  none was pre-picked.

  Dict and list literals (`classify_aggregate_part` in
  [`dispatch/literal.rs`](../../src/machine/execute/dispatch/literal.rs))
  ride the same name-resolve rail when their `wrap_identifiers` plan-input
  is set: bare-name entries call `resolve_name_part` directly and
  materialize as `Slot::Static` (resolved) or `Slot::Park(i)` (parked
  producer), with the dep-finish driving a single wake across all parked
  siblings.

`Resolved.slots`'s three index vectors (`wrap_indices` / `ref_name_indices` /
`eager_indices`) are disjoint by construction: each slot's
`(SignatureElement, ExpressionPart)` shape lands in at most one bucket.
[`KFunction::classify_for_pick`](../../src/machine/core/kfunction.rs) is
the sole producer of the `ClassifiedSlots` carrier (which `Resolved` holds
by value), so the disjointness invariant lives in one place rather than as
comment-enforced rules across the scheduler driver. Cycle detection runs
inside the fused walk (not in the cache build) so it sees the picked
function's slot classification: a binder declaration slot — `x` in
`LET x = …`, `Foo` in `NEWTYPE Foo (…)` — is owned by the binder, never
classified as wrap or ref-name, and so never reaches the cycle-check arm.
`DepGraph::would_create_cycle` walks the forward `notify_list` graph from
the consumer; if the producer is reachable, the driver surfaces
`SchedulerDeadlock` on the slot terminal instead of installing a park edge
that would close the cycle. That catches the trivially-cyclic
`LET Ty = Ty` / `LET x = x` shapes uniformly — both Identifier-LHS and
Type-LHS cycles surface with the same error kind without a special case
in the elaborator.

A bare-identifier slot resolving to a producer returns `Outcome::Forward` and is
spliced out (above). The other parking fast-lane handlers (the `fn_value`
`FunctionValueCall` head-placeholder park) and the eager-resolve pass return a
`ParkThenContinue` with a `Continuation::Resume` for a re-resolve, whose harness
calls `DepGraph::add_park_edge` — recording a `DepEdge::Notify(producer)` in the
consumer's `dep_edges` entry alongside the `DepEdge::Owned(child)`
entries that mark sub-slots the consumer owns. The bare-name splice likewise wires
the moved consumers through `add_park_edge` against the resolved producer. `add_park_edge` and its
`add_owned_edge` sibling each install the forward `notify_list[producer]`
wake and the `pending_deps[consumer]` bump atomically with the backward
record, so a park-edge install is one atomic +1 across the three vectors.
`free()` recurses only into `Owned` arms, so a consumer's reclamation
cannot transit a park edge into a sibling producer's subtree. Same-scope
rebind of a value name surfaces as `KErrorKind::Rebind`; an `FN` overload
duplicating an existing exact signature surfaces as
`KErrorKind::DuplicateOverload`. Type bindings share this placeholder
mechanism: a type-binding site registers in `Scope::placeholders` exactly
like a value binding, external lookups park the same way, and
self-references during a binding's own elaboration short-circuit through
the elaborator's threaded-set recognition (see
[typing/elaboration.md](../typing/elaboration.md)) so recursive type
definitions don't deadlock on their own placeholder. FN-signature
elaboration plugs into the same mechanism: when
[`elaborate_type_expr`](../../src/machine/model/types/resolver.rs) hits a
bare type-name leaf whose binder is in `Scope::placeholders` but not yet
finalized, it returns `ElabResult::Park(producers)` and FN-def's body
schedules a dep-finish over those producers that re-runs the signature
elaboration against the now-final scope at finish time. (See
[typing/elaboration.md § Layers](../typing/elaboration.md#layers) § Layer 3
for the elaborator's role in the pipeline.) A parens-wrapped
parameter type (`xs :(LIST OF Number)`) rides the same dep-finish:
`parse_fn_param_list` records the `(slot_idx, sub_expr)` pair, FN-def
schedules each sub-expression as its own sub-Dispatch, and the dep-finish's
finish closure splices each result into
`signature_expr.parts[slot_idx]` as `Future(Carried::Type(_))` before
re-running the parameter-list walk against the spliced signature. NEWTYPE
and UNION share the same elaborator-and-dep-finish shape for their
field-type lists. The fused walk's per-park cycle check
([`DepGraph::would_create_cycle`](../../src/scheduler/dep_graph.rs),
covered above) handles the simple trivially-cyclic cases proactively; the
elaborator's threaded-set carry-through handles the recursive-type cases
during NEWTYPE / UNION body elaboration.

A drain-end guard catches any cycle the proactive check doesn't: after
[`execute`](../../src/machine/execute/run_loop.rs) empties its work
queues, it scans the slot table for nodes still parked (`PreRun`) — a
node parked on a dependency that can no longer fire — and returns
`KErrorKind::SchedulerDeadlock { pending, sample }` rather than letting
the top-level result read panic on an unresolved slot. `sample` is the carrier
summary of the first parked node that has one (a dispatch decide carries its
expression's pre-rendered summary; a carrier-less dep-finish/catch wait falls back
to a generic tag), so the diagnostic points at code the reader can act on.

### Dispatch birth and resume

A dispatch slot is the one [`NodeWork`](../../src/machine/execute/nodes.rs) shape with
a decide `cont` (built by [`ignore_results`](../../src/machine/execute/outcome.rs))
and a `carrier` deadlock-summary string. The `cont` captures a
`SchedulerView -> Outcome` closure that reads the view, classifies / re-resolves,
and returns an `Outcome`; it takes no dep values, so its deps are park-only. Birth
and resume are the same shape, run through the same handler
([`run_step`](../../src/machine/execute/run_loop.rs)); the scheduler never
switches on dispatch-internal state and `NodeWork` names no `KExpression`.

**Birth** closures are built by the dispatch layer
([`decide`](../../src/machine/execute/dispatch.rs) / `submit_dispatch`) capturing the
slot's `expr` (+ `pre_subs`). On first poll the closure runs `classify_dispatch`,
which classifies `expr` via `classify_dispatch_shape` and decides against a
`SchedulerView`, returning an `Outcome`. `pre_subs` carries any recursively
pre-submitted sub-Dispatches keyed by their slot index in `expr.parts`, populated
at submit time for binder-shaped expressions so a nested binder's placeholders
install at the outermost submission point; `classify_dispatch` reuses these instead
of allocating fresh sub-Dispatches.

When a decide must wait — a keyworded resolve that found bare-name or
overload producers, a `FunctionValueCall` head still resolving to a
`Placeholder`, a `TypeCall` parked on a still-finalizing head — it returns a
`ParkThenContinue` whose continuation is a `Continuation::Resume` carrying an
opaque [`ResumeFn`](../../src/machine/execute/dispatch.rs) closure
(`SchedulerView -> Outcome`, built by `park_resume`). The harness parks the
slot's edges and installs a fresh **resume** decide carrying that closure. On
wake, `run_step` clears the slot's stale dep edges, runs the captured closure
against a fresh `SchedulerView`, and applies its `Outcome` — **one uniform arm**
for every shape. Clearing on resume is uniform and safe: a dispatch park installs
only `Notify` edges (sibling forward references, never children), which drop at
free, so a resume re-deriving its producers from the rebuilt scope cannot drop a
live wake. (Clearing on a fresh birth is a no-op — it owns no dep edges yet.)

Each family's closure captures exactly what its decide needs and re-runs it
against the now-populated scope:

- A **keyworded** bare-name park re-enters against the carried (partly-spliced)
  `working_expr`; the bare names now resolve through `scope.resolve_with_chain`
  to bound values, so the rebuilt `bare_outcomes` picks them up and the
  wrap-slot splice fires `Future(obj)` on the second pass.
- A keyworded **overload** park carries the original (unspliced) expression and
  re-runs the resolve against the now-populated `pending_overloads` bucket.
  **Eager subs never park here**: a `Deferred`/eager-subs resolve returns a
  `ParkThenContinue` with a `Continuation::Finish` and parks on a node with a
  dep-finish `cont` whose finish re-resolves the spliced expression — so a
  keyworded resume never re-enters for them. Re-resolve in the finish is
  authoritative: an element-typed `Future(_)` that narrows a typed-slot
  admission rules a speculative initial pick out, and the call surfaces
  `DispatchFailed` (non-match) rather than committing to a bind-time
  `TypeMismatch`.
- A **`FunctionValueCall`** head-placeholder park (`fn_value::install_head_park`)
  carries the original call expression and re-runs the fast lane once
  `scope.resolve_with_chain` lands in the `Resolution::Value` arm. Its eager
  subs route through `apply_callable::install_eager_subs_track`, which returns
  a `Continuation::Finish` carrying the picked `KFunction` from the head directly;
  `FunctionValueCall` is non-overload-set, so a typed `Future(_)` an eager sub
  reveals can't narrow the pick and the finish binds `picked` without
  re-resolving.

**Park exclusivity holds by construction.** A single resolve reaches exactly
one park installer: the overload park installs from a resolve failure *before*
the part walk runs; the bare-name park installs *before* any eager sub could
stage, because the part walk's park-precedence guard runs first; eager subs
take the dep-finish route rather than a resume. So a slot's resume
carries exactly one park reason.

The drain-end cycle-detection guard (`NodeStore::unresolved`) summarizes parked
slots from each `NodeWork`'s `carrier` — a dispatch decide carries its
expression's pre-rendered summary; a carrier-less dep-finish/catch wait falls back to
a generic `<wait>` tag — selected by a testable `work_deadlock_sample` helper in
`node_store`.

