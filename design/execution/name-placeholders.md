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

The mechanism lives in two install channels, one per binder shape. Which
channel a binder uses — and the name or bucket it declares — is read
**parse-statically**: every [`KExpression`](../../src/machine/model/ast.rs)
caches, beside its `DispatchShape`, the set of binders its subtree installs into
the enclosing scope ([`binder_installs`](../../src/machine/model/ast.rs), per the
position rule below). The single source of truth for which AST forms introduce a
binder, and which name or bucket each declares, is the static
[`BINDER_SPECS`](../../src/machine/model/binder.rs) table, keyed by untyped
signature shape and pinned against the live builtin table by a spec⟺registration
consistency test.

A `placeholders` table — a `RefCell<HashMap<String, (NodeId, BindingIndex)>>`
— lives inside the [`Bindings`](../../src/machine/core/bindings.rs) façade
on `Scope` alongside `data`, `types`, `functions`, and
`pending_overloads`. *Name-keyed binders* (`LET`, `TYPE`, `MODULE`, `GROUP`,
`SIG`, `UNION`, `NEWTYPE`, `RECURSIVE TYPES`) declare a
[`BinderKey::Name`](../../src/machine/model/binder.rs) — the to-be-bound name the
matching spec's extractor pulls structurally out of the expression's parts —
stamping `name → producer NodeId` paired with the binder's
[`BindingIndex { idx }`](../../src/machine/core/bindings.rs) — the lexical
statement index, gated by the strict `idx < cutoff` rule like every other
binder.

*Bucket-keyed binders* (`FN`, `OP`) declare a
[`BinderKey::Bucket`](../../src/machine/model/binder.rs) — every inner-call
bucket key a call to the to-be-registered overloads would compute — into a
separate `pending_overloads` table — a
`RefCell<HashMap<UntypedKey, Vec<(NodeId, BindingIndex)>>>` keyed by
the inner-call bucket key so a later-arriving call expression can park
on a not-yet-finalized overload. A named `FN` / `OP` uses the bucket channel,
never the name channel, because sibling
overloads under one head keyword (e.g. two `FN (PICK xs :A) ...` /
`FN (PICK xs :B) ...` declarations) must not collide on a single
`placeholders[name]` slot. The two channels are mutually exclusive per
binder: each binder uses exactly one, and the
[`BinderKey`](../../src/machine/model/binder.rs) enum
(`Name(String, BindKind)` vs. `Bucket(Vec<UntypedKey>)`) makes the dichotomy a
type-level fact rather than a two-Option convention.

The bucket vec is what admits multiple sibling FN binders
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

Binder builtins opt in through the `binder: bool` flag they pass to
[`register_builtin_full`](../../src/builtins.rs) (`LET`, `TYPE`, `MODULE`,
`GROUP`, `SIG`, `UNION`, `NEWTYPE`, `RECURSIVE TYPES`, `FN`, `OP`); the flag is
only the classification bit dispatch reads — a binder's literal-name slots are
declarations, not references, so they must not replay-park on their own
placeholder — while the name or bucket each installs lives once in the
[`BINDER_SPECS`](../../src/machine/model/binder.rs) table. `VAL` is a declaration
form that installs nothing; everything else stays placeholder-free.

A placeholder is keyed by `BindKind` (value or type), and each binder's kind is
fixed by the name part its binder-name extractor reads: `type_part_binder_name`
(SIG / UNION / NEWTYPE / RECURSIVE TYPES) reads a `Type` part and tags
`BindKind::Type`; `identifier_part_binder_name` (`LET <name> = …`, `MODULE`) reads
an `Identifier` part and tags `BindKind::Value`. `MODULE` binds a value under a
value token, so its placeholder and its write sit on the same ladder — no binder
straddles the two kinds, and no write clears a placeholder of the other kind
([`Bindings::try_apply`](../../src/machine/core/bindings.rs)). A spec's extractors
run in order and the first `Some` wins, so an expression whose name part is of one
class selects the correctly-classified channel — the value extractor misses a
`Type` part, and vice versa
([modules.md § First-class modules](../typing/modules.md#first-class-modules)).

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

### Submission-time binder install and the position rule

Binder discovery is parse-static, so submission does no AST recursion. Every
node caches [`binder_installs`](../../src/machine/model/ast.rs) — the aggregate
of every binder its subtree installs into the enclosing scope, computed bottom-up
in `fill_cache` from the [`BINDER_SPECS`](../../src/machine/model/binder.rs)
table. The dispatch-layer submission chokepoint
[`KoanRuntime::submit_expression`](../../src/machine/execute/dispatch/submit.rs)
reads that aggregate **once**, for a statement submission, and stamps each
entry's `placeholders[name]` or `pending_overloads[bucket]` entry on the
dispatching scope — with the enclosing statement's freshly allocated node id and
`BindingIndex::value(chain.index)` — before the slot is ever popped from the work
queues. A later sibling that dispatches before the statement's slot pops finds the
entry and parks rather than surfacing `UnboundName` / `DispatchFailed`. There is
exactly one install site, at statement submission; nothing installs at
dispatch/pick time. The binder logic lives in the dispatch layer, not the
scheduler: the scheduler exposes only a generic slot allocator
(`Scheduler::alloc_node`) and the `Scope::install_*` primitives, so no `NodeWork`
variant or scheduler code names a `KExpression`.

Because the aggregate is keyed to the enclosing statement, a nested binder's
placeholder carries the *statement's* node id, not the inner sub-dispatch's — a
sibling parked on a nested binder wakes when the whole statement completes. A
top-level binder is unchanged: its own node is the statement.

**The position rule.** A binder may appear only where a parse-static install is
sound:

- **statement position** — a top-level line, or a statement of a module / `FN` /
  `GROUP` body (each body statement submits as its own statement);
- **a lazily-captured body** — a `:KExpression` slot, whose statements install at
  their own block entry, not in the enclosing aggregate;
- **another binder's own declaration slot** — the eager value slot of an
  enclosing binder (`LET f = (FN …)`, `LET z = (LET a = 3)`), staged with the
  `binder_covered` bit set (in [`keyworded.rs`](../../src/machine/execute/dispatch/keyworded.rs))
  so the aggregate the enclosing statement already installed covers it;
- **a redundant single-`Expression` paren wrapper** — `((…))` passes its child's
  aggregate straight through.

Every other eagerly-dispatched position — a user-call or builtin argument, an
operator operand, a list / dict / record literal element, a deferred head — is an
error. When such a sub-dispatch's cached aggregate is non-empty and the dep was
not `binder_covered`, `submit_expression` allocates the slot pre-errored with
[`KErrorKind::NestedBinder`](../../src/machine/core/kerror.rs): slot-terminal and
TRY-catchable, it propagates through the dep like any other failed dep. The rule
covers **every** binder form — name-installing declarations and named `FN` / `OP`
definitions alike; a named `FN` / `OP` in an eager value position is the same
error, not a value whose registration silently vanishes. The value route is the
anonymous `FN :{…}` form (which installs nothing) or a name bound through a legal
binder chain.

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

