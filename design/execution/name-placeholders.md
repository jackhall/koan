# Name placeholders and submission

Forward-reference name placeholders that let a consumer park on a not-yet-bound
producer, the Miri lifetime contract for the splice/replay, and submission-time
binder install. The submit side of the dispatch pipeline; the execute side is
[Classify and apply](classify-and-apply.md). Part of the
[execution model](README.md).

## Dispatch-time name placeholders

Forward references between sibling top-level expressions, members of a
`MODULE` body, and (eventually) names imported across files all require the
same property: a value- or type-position lookup whose target binder has
dispatched but not yet executed parks on the producer instead of failing with
`UnboundName` — **provided the binding is lexically visible from this
reference's source position.** Visibility is the index gate (see
[Lexical provenance chain](calls-and-values.md#lexical-provenance-chain) below): every binding
carries the lexical statement index it was registered at, and a consumer at
chain cutoff `c` sees only bindings with index `i < c`. This is one rule
across the value and type languages — there is no per-binding exemption.
Mutual recursion of two or more nominal types, which has no valid source
order, is co-declared in a `RECURSIVE TYPES` block that scopes its threaded
group within strict lexical order (see
[typing/user-types.md](../typing/user-types.md)); a self-recursive type threads
its own name and needs no block.

Every binder is value-style gated (strict `b.idx < c`), so a forward
reference to a later-sibling `LET`, `NEWTYPE`, `FN`, or any other binder is
invisible. A later-sibling `LET` surfaces `UnboundName`; a forward call to a
later-sibling `FN` overload surfaces `DispatchFailed` rather than parking on
the not-yet-finalized overload; a forward type reference is a position error.
A *keyword-headed* function call (`ID 7`) resolves through the
`functions` bucket, which applies the same per-overload visibility filter:
a later-sibling overload registered after this consumer's statement is
hidden, and dispatch falls through to outer scopes. Forward calls from a
function *body* are unaffected — bodies re-dispatch per call against the
body's lexical chain, by which point every sibling binder has registered.

The mechanism lives in two pieces, each routed through a separate install
channel keyed by the binder's shape.

A `placeholders` table — a `RefCell<HashMap<String, (NodeId, BindingIndex)>>`
— lives inside the [`Bindings`](../../src/machine/core/bindings.rs) façade
on `Scope` alongside `data`, `types`, `functions`, and
`pending_overloads`. *Name-keyed binders* (`LET`, `NEWTYPE`, `UNION`,
`SIG`, `MODULE`, `RECURSIVE TYPES`) install through their
[`binder_name`](../../src/machine/core/kfunction/body.rs) hook (a per-
`KFunction` extractor of type
[`BinderNameFn`](../../src/machine/core/kfunction/body.rs) that pulls the
to-be-bound name structurally out of the expression's parts), stamping
`name → producer NodeId` paired with the binder's
[`BindingIndex { idx }`](../../src/machine/core/bindings.rs) — the lexical
statement index, gated by the strict `idx < cutoff` rule like every other
binder.

*Bucket-keyed binders* (`FN`, `FUNCTOR`) install through a
[`binder_bucket`](../../src/machine/core/kfunction/body.rs) extractor
([`BinderBucketFn`](../../src/machine/core/kfunction/body.rs)) into a
separate `pending_overloads` table — a
`RefCell<HashMap<UntypedKey, Vec<(NodeId, BindingIndex)>>>` keyed by
the inner-call bucket key so a later-arriving call expression can park
on a not-yet-finalized overload. FN/FUNCTOR carry **only** the
`binder_bucket` extractor — no `binder_name` — because sibling
overloads under one head keyword (e.g. two `FN (PICK xs :A) ...` /
`FN (PICK xs :B) ...` declarations) must not collide on a single
`placeholders[name]` slot. The two channels are mutually exclusive per
binder: each binder uses exactly one. The submission walk reifies the
choice as a
[`BinderKey`](../../workgraph/src/scheduler/alloc.rs) enum
(`Name(String)` vs. `Bucket(UntypedKey)`) so the dichotomy rides in
the type rather than as a two-Option convention.

The bucket vec is what admits multiple sibling FN/FUNCTOR binders
sharing one bucket key: each install appends a distinct entry at its
own `BindingIndex`. A consumer looking up the bucket via
[`Bindings::lookup_function`](../../src/machine/core/bindings.rs) gets the
*earliest-index visible* `pending_overloads[key]` entry in the returned
`FunctionLookup`'s `pending` field — the most-likely-first-finalizer. On
that producer's finalize, only the matching entry is removed from the vec
(others stay pending); the consumer wakes, re-dispatches, and either picks
from the now-live `functions[bucket]` or re-parks on the next-earliest
pending sibling. Each re-dispatch is cheap, and the expected case
(consumer's match lands in the first 1–2 siblings) avoids the cost
entirely.

The six binder builtins (`LET`, `FN`, `NEWTYPE`, `SIG`, `UNION`,
`MODULE`) opt in via
[`register_builtin_with_binder`](../../src/machine/core/kfunction.rs);
everything else stays placeholder-free.

A placeholder is keyed by `BindKind` (value or type), and `MODULE` straddles the
two: a module name is a Type token, so the binder parks in-flight forward
references through the *type* ladder (`BindKind::Type`), while the module itself
binds value-side into `data` ([modules.md § First-class
modules](../typing/modules.md#first-class-modules)). The value write therefore
clears the Type-kind placeholder as well as the value-kind one
([`Bindings::try_apply`](../../src/machine/core/bindings.rs)); without that, a
forward reference like `x :View` would park forever on a producer that has already
run. The [naming flip](../../roadmap/type_memos/module-naming-flip.md) retires
Type-token module names and this cross-kind clear with them.

Production reads thread the three-layer
[lookup → admit protocol](../typing/lookup-protocol.md): `Scope::resolve_*_with_chain`
walks ancestors, the `Bindings::lookup_*` accessors apply the
`chain_cutoff`-gated `visible` predicate per entry, and `KType`
predicates accept or reject the candidate. The placeholder mechanism
extends the value- and function-side lookups so a still-running visible
producer surfaces as `NameLookup::Parked(NodeId)` /
`FunctionLookup { pending: Some(_), .. }` rather than a miss —
[`Bindings::lookup_value`](../../src/machine/core/bindings.rs) consults
`data` then `placeholders`, and
[`Bindings::lookup_function`](../../src/machine/core/bindings.rs) surfaces
the visibility-filtered `functions[key]` overloads and the earliest-index
visible `pending_overloads[key]` producer *together* in one
`FunctionLookup`. The dispatcher decides each scope's contribution from
that pair as it walks (a visible pending parks the scope; see
[scheduler.md § In-walk dispatch precedence](../typing/scheduler.md#in-walk-dispatch-precedence)),
so the bucket / pending-overload pair surfaces from one traversal rather
than two. The
raw map accessors (`data` / `types` / `functions` / `placeholders` /
`pending_overloads`) are gated `#[cfg(test)]`; production sites that
genuinely sweep all members (`MODULE` member mirroring, signature
shape-check, REPL reflection) consume the value-yielding `iter_data` /
`iter_types` / `iter_functions`, which release the underlying borrow at
the iterator boundary. `bind_value` and `register_function` remove their
own placeholder before inserting into `data` / `functions`, so the two
tables are mutually exclusive at any moment.

### Miri forward-splice and replay-park lifetime contract

A bare-name slot whose name resolves to a still-running producer is spliced out
as an alias of it (see [Bare-name forward splice](scheduler.md#bare-name-forward-splice)). A
read of the aliased slot resolves to the producer and returns the producer's own
`&KObject<'a>` reference — not a clone. The producer's region therefore must
outlive every consumer that reads through the alias. The replay-park route is
symmetric: a parked dispatch decide's captured scope, and the `&KObject<'a>` its
resolved producers carry, must stay valid across the wake and the re-dispatch.
The `lift_park_minimal_program_for_miri` (a bare-name forward, `LET y = z`) and
`replay_park_minimal_program_for_miri` (a parked-and-resumed FN call) tests pin
the contract under Miri tree borrows.

### Submission-time binder install and recursive sub-Dispatch

The dispatch-layer submission chokepoint
[`dispatch::submit_dispatch`](../../src/machine/execute/dispatch/submit.rs)
inspects every dispatch submission against the dispatching scope's ancestor
chain via `extract_binder_install`: it finds the first overload in the
matching `functions[expr.untyped_key()]` bucket whose `binder_name` OR
`binder_bucket` extractor returns `Some(_)` for the expression. The picked
overload's install channel is reified as `BinderKey::Name(name)` (for `LET` /
`NEWTYPE` / `UNION` / `SIG` / `MODULE`) or `BinderKey::Bucket(key)` (for `FN` /
`FUNCTOR`); the install site stamps the corresponding `placeholders[name]` or
`pending_overloads[bucket]` entry on the dispatching scope before the slot is
ever popped from the work queues. A later sibling that dispatches before the
binder's slot pops finds the entry and parks rather than surfacing
`UnboundName` / `DispatchFailed`. The binder logic lives in the dispatch layer,
not the scheduler: the scheduler exposes only a generic slot allocator
(`Scheduler::submit_node`) and the `Scope::install_*` primitives, so no
`NodeWork` variant or scheduler code names a `KExpression`.

For binder-shaped expressions, `submit_dispatch` also recurses into the eager
Expression-shaped argument slots and submits each as a sub-dispatch *at the same
outermost submission point*. The walk computes an `eager_slot_mask` over the
bucket — a slot is eager only if *every* binder overload in the bucket marks it
non-`KType::KExpression`; any overload tagging a slot lazy keeps that slot out
of the recursive walk because the eventual dispatch may resolve to that
overload. Lazy slots — FN body, FN signature/return-type-`KExpression` overload,
FUNCTOR body, MODULE body — dispatch in the callee's scope at body-invoke time,
not here. Each recursive `submit_dispatch` runs its own
`extract_binder_install`, so a nested binder's placeholder installs at the same
outermost step as its parent's; recursion terminates at non-binder leaves and at
lazy slots, bounded by AST depth.

The collected `(slot_idx, sub_node_id)` pairs are captured (with `expr`) in the
parent's birth dispatch decide closure
([`decide_with_presubs`](../../src/machine/execute/dispatch.rs)). When the parent runs,
the fused splice / park / eager-sub walk in
[`dispatch.rs`](../../src/machine/execute/dispatch.rs) consults
`pre_subs` before the `Expression` / `ListLiteral` / `DictLiteral` arms:
a slot already pre-submitted reuses the existing `NodeId` (and replaces
the part with an empty-`Identifier` placeholder for the eventual expression
splice) rather than allocating a fresh sub-Dispatch. The
`KeywordedState::install_bare_name_park` and `install_overload_park`
installers carry `pre_subs` into the `KeywordedState.init.pre_subs`
field of the parked state, and `KeywordedState::resume` hands it back to
`initial` on wake — so a park-and-wake cycle does
not re-allocate the pre-submitted children.

Statement indices are per-`enter_block` call: each call to
[`KoanRuntime::enter_block`](../../src/machine/execute/runtime/submit.rs) mints
chain frames at indices `1..N` for the N statements it submits. A REPL
or test fixture that submits without an ambient chain (the
[`Scheduler::add`](../../workgraph/src/scheduler/alloc.rs) auto-root
branch) gets [`LexicalFrame::detached`](../../src/machine/core/lexical_frame.rs)
— a chain that mentions no real scope, so the visibility predicate's
`index_for → None ⇒ complete` arm makes every binding in the target
scope visible. This is what lets a REPL query read through to every
prior bind without sharing an index space with them.

