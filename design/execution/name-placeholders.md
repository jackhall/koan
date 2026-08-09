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

The mechanism lives in two install channels. Which channels a binder fills — and
the name and bucket keys it declares — is read **parse-statically**: every
[`KExpression`](../../src/machine/model/ast.rs) caches, beside its
`DispatchShape`, what it *itself* installs into the enclosing scope
([`binder_plan`](../../src/machine/model/ast.rs), per the position rule below) —
its own spine, never what its slots contain. The single source of truth for which
AST forms introduce a binder, and which name and buckets each declares, is the
static
[`BINDER_SPECS`](../../src/machine/model/binder.rs) table, keyed by untyped
signature shape and pinned against the live builtin table by a spec⟺registration
consistency test.

### A placeholder is a pending arm of the slot it resolves into

A placeholder has no table of its own. The
[`Bindings`](../../src/machine/core/bindings.rs) façade on `Scope` holds four
maps — `data`, `types`, `functions`, `operators` — and a still-finalizing binder
occupies **a slot of the very table it will resolve into**, as a
[`PendingBinding { producer, index }`](../../src/machine/core/bindings.rs) arm of
that slot. Three properties follow, and they are the reason for the shape: a name
lookup is answered by one probe; finalization overwrites the claimed slot in
place, so the key is stored once and the claim's bytes are never abandoned; and
the exclusivity rule each table obeys is a fact of its slot enum.

*Name-keyed binders* (`LET`, `TYPE`, `MODULE`, `GROUP`, `SIG`, `UNION`,
`NEWTYPE`, `RECURSIVE TYPES`) fill the **name channel** of the
[`BinderKey`](../../src/machine/model/binder.rs) — the to-be-bound name the
matching spec's name extractor pulls structurally out of the expression's parts.
The
claim stamps `producer NodeId` paired with the binder's
[`BindingIndex { idx }`](../../src/machine/core/bindings.rs) — the lexical
statement index — into `data[name]` or `types[name]` per the binder's
`BindKind`, gated by the strict `idx < cutoff` rule like every other binder. The
same visibility predicate therefore gates a pending arm and the binding it
becomes.

The two name-side tables carry different slot enums because they enforce
different rules:

- `data[name]` is a [`ValueSlot`](../../src/machine/core/bindings.rs) — `Bound`
  xor `Pending`. A value name is never bound and pending at once: the claim
  errors `Rebind` against a committed value, and the value write finalizes the
  claim.
- `types[name]` is a [`TypeSlot`](../../src/machine/core/bindings.rs) —
  `Bound`, `Pending`, or `BoundWithPending`. Bound and pending coexist here by
  design: a parallel nominal finalize pre-installs the name's external identity
  while its producer is still in flight, and the finalize gate must still park
  the type-identifier memo on that producer. The third arm makes the coexistence
  — and the impossibility of an empty slot — type-level facts. Reads go through
  the slot's `bound()` / `pending()` accessors; only the three transition sites
  (the type write, the claim, the producer-failure sweep) match the arms
  directly.

*Bucket-keyed binders* (`FN`, `OP`) fill the **bucket channel** — every
inner-call bucket key a call to the to-be-registered overloads would compute. A
pending
overload already keys on the same full `UntypedKey` as the overload it becomes,
so it lands in the bucket it resolves into: `functions[key]` is a `Vec` of
[`OverloadSlot`](../../src/machine/core/bindings.rs), each either `Sealed` or
`Pending`, and a bucket legitimately holds both at once. Keying by the full
bucket key is what keeps `(MAKESET _)` and `(MAKESET _ USING _)` from colliding.
A bare named `FN` / `OP` uses the bucket channel and not the name channel,
because sibling overloads under one head keyword (e.g. two `FN (PICK xs :A) ...`
/ `FN (PICK xs :B) ...` declarations) must not collide on a single name slot.

The two channels are two fields of one key, not two alternatives: a
[`StoredBinderKey`](../../src/machine/model/binder.rs) carries an optional
`(name, BindKind)` and an optional
[`BucketKeys`](../../src/machine/model/binder.rs) pair, so one statement may fill
either channel or both. The **combined statement forms** —
`LET <name> = FN <signature> -> <Return> = (<body>)` and the
`LET <name> = OP …` / `LET <name> = UNARY OP …` twins — fill both from a single
binder: the value name and the bucket key(s) the declaration's body registers
under. Two bucket keys is the maximum any form reaches (a `UNARY OP` declares the
keyword-first list key plus the binary bridge key), so the record is fixed-size
and `Copy`, its strings and key runs bumped into the declaring node's own region
with nothing heap-owned. The owned twin
[`BinderKey`](../../src/machine/model/binder.rs) is the transient currency the
submission path hands the bindings tables.

A combined form's two writes describe **one** `KFunction`: the finalize seals the
callable once and duplicates that cell into a `WriteOp::Value` beside the
`WriteOp::Overload`, both at the `BindingIndex` the submission-time claim
stamped. So the bound name and the registered overload are the same function by
construction, not two builds of the same source — a closure captured under the
name observes exactly what a keyworded call to it observes.

The bucket vec is what admits multiple sibling FN binders
sharing one bucket key: each install appends a distinct pending slot at its
own `BindingIndex`. A consumer looking up the bucket via
[`Bindings::lookup_function`](../../src/machine/core/bindings.rs) gets the
*earliest-index visible* pending slot in the returned `FunctionLookup`'s
`pending` field — the most-likely-first-finalizer. On that producer's finalize,
the seal lands in that binder's own pending slot, matched by `BindingIndex`
(others stay pending); the consumer wakes, re-dispatches, and either picks
from the now-live `functions[bucket]` or re-parks on the next-earliest
pending sibling. Each re-dispatch is cheap, and the expected case
(consumer's match lands in the first 1–2 siblings) avoids the cost
entirely. Slot order within a bucket is not observable: the picker returns the
signature that strictly dominates every other survivor, or a tie that surfaces
as deferred/ambiguous either way.

Bulk reads see bound state only. [`iter_data`](../../src/machine/core/bindings.rs)
/ `iter_types` / `iter_functions` and the module-view `bulk_install_from` skip
pending arms and skip a bucket holding no sealed slot: a claim names a producer
in its own scheduler run, so a copy of one would hand the target a park on a node
that will never wake it.

Binder builtins opt in through the `binder: bool` flag they pass to
[`register_builtin_full`](../../src/builtins.rs) (`LET`, `TYPE`, `MODULE`,
`GROUP`, `SIG`, `UNION`, `NEWTYPE`, `RECURSIVE TYPES`, `FN`, `OP`); the flag is
only the classification bit dispatch reads — a binder's literal-name slots are
declarations, not references, so they must not replay-park on their own
placeholder — while the name or bucket each installs lives once in the
[`BINDER_SPECS`](../../src/machine/model/binder.rs) table. `VAL` is a declaration
form that installs nothing; everything else stays placeholder-free.

A claim's `BindKind` (value or type) picks its destination table, and each
binder's kind is fixed by the name part its binder-name extractor reads:
`type_part_binder_name` (SIG / UNION / NEWTYPE / RECURSIVE TYPES) reads a `Type`
part and tags `BindKind::Type`; `identifier_part_binder_name` (`LET <name> = …`,
`MODULE`) reads an `Identifier` part and tags `BindKind::Value`. `MODULE` binds a
value under a value token, so its claim and its write sit on the same ladder — no
binder straddles the two kinds, and a write of one kind can never finalize the
other kind's claim, because the two live in different tables
([`WriteOp::apply`](../../src/machine/core/bindings/ops.rs)). Since the parser
tags a `Type` part only for a name that classifies as a Type token, and an
`Identifier` part only for one that does not, the two channels cannot even
contend for a name. A spec's name extractors
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
[`Bindings::lookup_value`](../../src/machine/core/bindings.rs) reads the arm of
the one `data[name]` slot it probes, and
[`Bindings::lookup_function`](../../src/machine/core/bindings.rs) surfaces
the visibility-filtered sealed overloads of `functions[key]` and the
earliest-index visible pending sibling in that same bucket *together* in one
`FunctionLookup`. The dispatcher decides each scope's contribution from
that pair as it walks (a visible pending parks the scope; see
[scheduler.md § In-walk dispatch precedence](../typing/scheduler.md#in-walk-dispatch-precedence)),
so the sealed / pending pair surfaces from one traversal rather
than two. `lookup_type` prefers its slot's bound arm over its pending one, which
is load-bearing: on a slot carrying both, a consumer that can read the identity
must not park. The
raw map accessors (`data` / `types` / `functions`) and the pending probes
(`pending_value` / `pending_names` / `pending_overload_entries`) are gated
`#[cfg(test)]`; production sites that
genuinely sweep all members (`MODULE` member mirroring, signature
shape-check, REPL reflection) consume the value-yielding `iter_data` /
`iter_types` / `iter_functions`, which release the underlying borrow at
the iterator boundary. `bind_value` and `register_function` finalize their own
claim by overwriting the slot that holds it, so no name is ever both bound and
claimed on the value side, and a bucket's sealed entry sits where its claim was.

The error path is the one place a claim dies without a write. When a producer's
node finalizes with an error, `clear_placeholders_for_producer` drops every
pending arm naming it — a `types` slot that also holds a bound
identity keeps the identity and loses only its pending arm — so a binder body
that failed before its write path cannot leak a scheduler-local `NodeId` into a
later run on a persistent scope. The sweep keys on the `producer` every
`PendingBinding` already carries, so it spans all three claim-bearing tables
alike: no table's key participates, and a bucket-keyed binder's claim dies in
every inner-call bucket it declared, the emptied bucket losing its key. A
finalize therefore only ever overwrites its own live claim — the write path has
no leftover-claim cleanup of its own, because the error path leaves none behind.

### Miri forward-splice and replay-park lifetime contract

A bare-name slot whose name resolves to a still-running producer is spliced out
as an alias of it (see [Bare-name forward splice](scheduler.md#bare-name-forward-splice)). A
read of the aliased slot resolves to the producer and returns the producer's own
`&KObject<'a>` reference — not a clone. The producer's region therefore must
outlive every consumer that reads through the alias. The replay-park route is
symmetric: a parked dispatch decide's captured scope, and the `&KObject<'a>` its
resolved producers carry, must stay valid across the wake and the re-dispatch.
The `park_and_replay_minimal_program_for_miri` test pins both halves of the
contract under Miri tree borrows in one batch-submitted program: `LET y = z` is
the bare-name forward, and `LET out = (DOUBLE y)` is the FN call parked on that
same binding and replayed on the wake.

### Submission-time binder install and the position rule

Binder discovery is parse-static and **per-statement**, so submission does no AST
recursion. Every node caches
[`binder_plan`](../../src/machine/model/ast.rs) — what that node itself installs
into the enclosing scope, read at construction from the
[`BINDER_SPECS`](../../src/machine/model/binder.rs) table and `None` for a node
that is not a binder. The dispatch-layer submission chokepoint
[`KoanRuntime::submit_expression`](../../src/machine/execute/dispatch/submit.rs)
reads that plan **once**, for a statement submission, and stamps its claims — a
pending arm of `data[name]` / `types[name]` for the name channel, and a pending
slot appended to `functions[bucket]` for each bucket key — on the dispatching
scope, with the statement's freshly allocated node id and
`BindingIndex::value(chain.index)`, before the slot is ever popped from the work
queues. A later sibling that dispatches before the statement's slot pops finds the
entry and parks rather than surfacing `UnboundName` / `DispatchFailed`. There is
exactly one install site, at statement submission; nothing installs at
dispatch/pick time. The binder logic lives in the dispatch layer, not the
scheduler: the scheduler exposes only a generic slot allocator
(`Scheduler::alloc_node`) and the `Scope::install_*` primitives, so no `NodeWork`
variant or scheduler code names a `KExpression`.

A statement's plan is its own spine and nothing else, so the namespace a block
introduces is legible from its statement keys alone — which is what
order-independent sibling submission needs. A combined form stamps both channels
at that one node id and one `BindingIndex`, so a sibling parked on the name and a
sibling parked on the bucket wake on the same statement.

**The position rule.** Binding is a statement-level act. A binder may appear only
where a parse-static install is sound:

- **statement position** — a top-level line, or a statement of a module / `FN` /
  `GROUP` body (each body statement submits as its own statement);
- **a lazily-captured body** — a `:KExpression` slot, whose statements install at
  their own block entry;
- and, structurally, **a redundant single-`Expression` paren wrapper** — `((…))`
  is the same statement, so the submission path reads its child's plan through it.

Every other eagerly-dispatched position — a user-call or builtin argument, an
operator operand, a list / dict / record literal element, a deferred head, and
**another binder's own value slot** (`LET f = (FN …)`, `LET z = (LET a = 3)`) —
is an error. When such a sub-dispatch carries a plan, `submit_expression`
allocates the slot pre-errored with
[`KErrorKind::NestedBinder`](../../src/machine/core/kerror.rs): slot-terminal and
TRY-catchable, it propagates through the dep like any other failed dep. The rule
covers **every** binder form — name-installing declarations and named `FN` / `OP`
definitions alike; a named `FN` / `OP` in an eager value position is the same
error, not a value whose registration silently vanishes. The value route is the
anonymous `FN :{…}` form, which installs nothing; a definition that must also
bind a name is one statement in the combined spelling, which the error message
names when the rejected node registers overloads.

The combined form is a single *statement*, which is not the same as a single
line: a line ending in `,` continues the statement, while a bare indented
continuation wraps the remainder in a nested expression — putting the definition
back in a value slot and re-earning the error. `LET f = ,` then the indented
`FN …` is one statement; without the comma it is two nodes and an error.

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

