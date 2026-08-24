# Name placeholders and submission

Name claims that let a consumer park on an earlier binder that has dispatched
but not yet bound, the Miri lifetime contract for the splice/replay, and
submission-time binder install. The submit side of the dispatch pipeline; the execute side is
[Classify and apply](classify-and-apply.md). Part of the
[execution model](README.md).

## Dispatch-time name placeholders

References between sibling top-level expressions, members of a
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
order, is co-declared in a module body, whose top-level type declarations are
announced before any of them runs and are therefore mutually visible within that body
(see [typing/user-types.md](../typing/user-types.md)); a self-recursive type threads
its own name and needs no wrapper.

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

### A claim lives in the scope's claim store

A claim is not a table entry. The
[`Bindings`](../../src/machine/core/bindings.rs) façade on `Scope` holds the four
binding maps — `data`, `types`, `functions`, `operators` — beside a **claim
store** that holds nothing else: the in-flight binders of the one block that
binds into this scope. The binding maps therefore carry committed bindings only,
and each slot type states its own table's exclusivity rule with no in-flight arm
to admit.

The store ([`claims.rs`](../../src/machine/core/bindings/claims.rs)) has three
parts, each answering one question. A `Claim` is the pair (`ProducerId`,
`BindingIndex`) throughout:

- `by_name` — the name's label [`Symbol`](../../src/machine/model/labels.rs) → `Claim`.
  The name channel's read path, and a name admits at most one claim. One map covers value
  and type claims alike — a claim is stamped before its producer's kind has settled — and
  it stays sound because the two bindable token classes name disjoint text
  ([label-interning.md § Classified label vocabulary](../label-interning.md#classified-label-vocabulary)).
- `by_bucket` — bucket key → a **run** of `Claim`, in install order. The bucket
  channel's read path. The value is a run and not a single claim because sibling
  binders legitimately share one bucket key, each claiming at its own
  `BindingIndex`; the read takes the earliest-index *visible* claim in the run.
- `by_statement` — a run sized at the block fan-out and indexed by
  `BindingIndex`, each entry naming the at-most-three keys its statement claimed
  plus a live mask over them. The retirement path, and the only part keyed by
  something other than what a reader looks up.

Each read path is addressed by one hash probe, which is what the resolution walk
needs: a claim is consulted on the miss that would otherwise raise
`UnboundName`, once per ancestor scope. (The bucket probe then walks the run it
lands in — bounded by the sibling binders declaring that one key, never by the
statement run.) The store lives inside `Bindings` rather than beside it on
`Scope` because a consumer may park on an in-flight binder in an *outer* scope —
the walk probes each ancestor's `bindings` gated by that scope's cutoff, so a
claim store anywhere else would be invisible to the ancestor probe.

**The block fan-out is the one act that builds a store**, and it is also where a
block rules on duplicate declarations. Every statement's binder plan is its own
spine, so a block's whole namespace is legible from the statement keys alone
before any statement runs: the fan-out sizes `by_statement` to the statement
count and, in the same pass, rejects a statement whose declared name an earlier
statement of the block already declared. That rejection is
`KErrorKind::DuplicateDeclaration`, which names *both* declaring positions, and
the rejected statement submits already-terminal — it never runs a body. Deciding
it here is what makes the diagnostic deterministic: which of two colliding
statements is rejected is a fact about lexical order, not about which body
happened to finish first. The **bucket** channel is deliberately exempt —
sibling overloads under one head keyword are the point of that channel, so a
shared bucket key is a co-declaration, and per-signature collisions surface as
`DuplicateOverload` at seal time where the signatures exist.

A claim's currency is an opaque
[`ProducerId`](../../src/machine/execute/producer_id.rs) over the claim edge,
not a node identity: the binder's submission wires an edge from its own slot
toward **the region of the scope the name is being introduced into**, and stamps
that edge's producer token into the claim store. A consumer that finds the
claim parks by
wiring its *own* edge off it (spending the token through the drive loop's
single verb), inheriting the destination — which is what makes a
placeholder park deliver into the scope the binding lives in rather than into the
consumer's region ([scheduler.md § Which edges Koan installs](scheduler.md#which-edges-koan-installs)).
The claim edge is owned by the slot that installed it, which releases it when it
terminalizes, so a table can never hold a name whose edge is gone.

*Name-keyed binders* (`LET`, `TYPE`, `MODULE`, `GROUP`, `SIG`, `UNION`,
`NEWTYPE`) fill the **name channel** of the
[`BinderKey`](../../src/machine/model/binder.rs) — the to-be-bound name the
matching spec's name extractor pulls structurally out of the expression's parts.
The
claim stamps the binder slot's own `ProducerId` paired with its
[`BindingIndex { idx }`](../../src/machine/core/bindings.rs) — the lexical
statement index — into `by_name`, gated by the strict `idx < cutoff` rule like
every other binder. The same visibility predicate therefore gates a claim and the
binding it becomes.

The two name-side channels differ only in which table a commit lands in, and the
store does not distinguish them. Bound-and-claimed coexistence needs no
representation: on the type side a nominal's seal pre-installs the name's
external identity into `types` while its producer is still in flight, and the
finalize gate must still park the type-identifier memo on that producer. That is
simply `types[name]` bound *and* a live `by_name` entry — two structures, each
answering its own question, so a consumer that can read the identity reads it
while the memo still finds the producer.

*Bucket-keyed binders* (`FN`, `OP`) fill the **bucket channel** — every
inner-call bucket key a call to the to-be-registered overloads would compute. A
claim keys on the same full `UntypedKey` as the overload it becomes, so
`by_bucket[key]` and `functions[key]` are reached by one key: the claim answers
"a binder for this bucket is in flight" and the bucket answers "these overloads
are registered", and a dispatch walk consults both at a scope. Keying by the full
bucket key is what keeps `(MAKESET _)` and `(MAKESET _ USING _)` from colliding.
A bare named `FN` / `OP` uses the bucket channel and not the name channel,
because sibling overloads under one head keyword (e.g. two `FN (PICK xs :A) ...`
/ `FN (PICK xs :B) ...` declarations) must not collide on a single name slot.

The two channels are two fields of one key, not two alternatives: a
[`StoredBinderKey`](../../src/machine/model/binder.rs) carries an optional
[`BinderSymbol`](../../src/machine/model/labels.rs) — `Value(ValueSymbol)` or
`Type(TypeSymbol)`, either way the symbol the parser minted when it classified the
token — and an optional
[`BucketKeys`](../../src/machine/model/binder.rs) pair, so one statement may fill
either channel or both. The variant *is* the bind kind, so the record states no
separate kind tag beside the name. The **combined statement forms** —
`LET <name> = FN <signature> -> <Return> = (<body>)` and the
`LET <name> = OP …` / `LET <name> = UNARY OP …` twins — fill both from a single
binder: the value name and the bucket key(s) the declaration's body registers
under. Two bucket keys is the maximum any form reaches (a `UNARY OP` declares the
keyword-first list key plus the binary bridge key), so the record is fixed-size
and `Copy`: a name of either class is a lifetime-free symbol, a key run is a borrow
into the declaring node's own region, and nothing is heap-owned. `to_owned_key`
mints nothing — both channels arrive already classified and interned. The owned twin
[`BinderKey`](../../src/machine/model/binder.rs) is the transient currency the
submission path hands the bindings tables.

A combined form's two writes describe **one** `KFunction`: the finalize seals the
callable once and duplicates that cell into a `WriteOp::Value` beside the
`WriteOp::Overload`, both at the `BindingIndex` the submission-time claim
stamped. So the bound name and the registered overload are the same function by
construction, not two builds of the same source — a closure captured under the
name observes exactly what a keyworded call to it observes.

One bucket key admits multiple sibling FN binders: each install adds a distinct
claim at its own `BindingIndex`. A consumer looking up the bucket via
[`Bindings::lookup_function`](../../src/machine/core/bindings.rs) gets the
*earliest-index visible* claim in the returned `FunctionLookup`'s
`pending` field — the most-likely-first-finalizer. On that producer's finalize,
the seal appends to `functions[bucket]` and that binder's claim retires (the
siblings' claims stand); the consumer wakes, re-dispatches, and either picks
from the now-live `functions[bucket]` or re-parks on the next-earliest
claiming sibling. Each re-dispatch is cheap, and the expected case
(consumer's match lands in the first 1–2 siblings) avoids the cost
entirely. Slot order within a bucket is not observable: the picker returns the
signature that strictly dominates every other survivor, or a tie that surfaces
as deferred/ambiguous either way.

Bulk reads see bound state only, and get it for free:
[`iter_data`](../../src/machine/core/bindings.rs) / `iter_types` /
`iter_functions` and the module-view `bulk_install_from` read the binding tables,
which hold nothing else. A claim names an edge of its own scheduler run, so a
copy of one would hand the target a park on an edge that will never wake it — and
that its owner has already released; keeping claims out of the tables is what
makes that unrepresentable rather than filtered.

Binder builtins declare themselves through the `binder: bool` flag they pass to
[`register_builtin_full`](../../src/builtins.rs) (`LET`, `TYPE`, `MODULE`,
`GROUP`, `SIG`, `UNION`, `NEWTYPE`, `FN`, `OP`); dispatch itself reads
binder-ness off the *expression*'s cached spec-table facts — the flag is the
registration-side declaration of the same fact, pinned against the
[`BINDER_SPECS`](../../src/machine/model/binder.rs) table (where the name or
bucket each form installs lives once) by the spec⟺registration consistency
test. `VAL` is a declaration
form that installs nothing; everything else stays placeholder-free.

A claim's `BindKind` (value or type) picks its destination table, and each
binder's kind is fixed by the name part its binder-name extractor reads:
`type_part_binder_name` (SIG / UNION / NEWTYPE) reads a `Type`
part and yields `BinderSymbol::Type`; `identifier_part_binder_name` (`LET <name> = …`,
`MODULE`) reads an `Identifier` part and yields `BinderSymbol::Value`. The kind is read
back off the variant, so a spec cannot declare one kind and extract the other. `MODULE` binds a
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
producer surfaces as `NameLookup::Parked(ProducerId)` /
`FunctionLookup { pending: Some(ProducerId), .. }` rather than a miss —
[`Bindings::lookup_value`](../../src/machine/core/bindings.rs) probes
`data[name]`, then the store; and
[`Bindings::lookup_function`](../../src/machine/core/bindings.rs) surfaces
the visibility-filtered sealed overloads of `functions[key]` and the
earliest-index visible claim on that same key *together* in one
`FunctionLookup`. The dispatcher decides each scope's contribution from
that pair as it walks (a visible claim parks the scope; see
[scheduler.md § In-walk dispatch precedence](../typing/scheduler.md#in-walk-dispatch-precedence)),
so the pair surfaces from one traversal rather than two. `lookup_type` prefers a
bound identity over a live claim on the same name, which is load-bearing: where
both stand, a consumer that can read the identity must not park. The
raw map accessors (`data` / `types` / `functions`) are gated `#[cfg(test)]`;
production sites that genuinely sweep all members (`MODULE` member mirroring,
signature shape-check, REPL reflection) consume the value-yielding `iter_data` /
`iter_types` / `iter_functions`, which release the underlying borrow at
the iterator boundary.

**A commit retires its own claim.** `write_value`, `write_overload` and
`write_type` each already carry the name (or bucket key) they are writing and the
`BindingIndex` they are writing it at — `write_type` through its
`DeclarationSite`. So a commit removes its own claim from the store's read map
and clears that channel's bit in `by_statement`: one hash removal and one bit,
with nothing searched for. On the success path there is no leftover claim, and
the write path needs no cleanup pass of its own.

**Claim retirement rides the slot's death, not just the error path.** A slot
owns every claim edge its submission stamped, and the
[`Workload::retiring`](../../workgraph/src/scheduler/workload.rs) hook retires
that list at the one point the slot stops being able to release it — invoked
by the scheduler exactly once per slot, covering every terminal (value,
error, and the bare-name forward's relocation alike) and the alias splice that
retires the slot without a terminal. Retirement drops whatever claims the commit
did not, then releases the edges themselves, so no store ever holds a name whose
edge is gone. The edge list is taken, not read, so a slot's edges release exactly
once.

Retirement is keyed by the one thing the retiring slot knows about itself — its
`BindingIndex`. It indexes `by_statement`, reads the live mask, and is done: a
zero mask is the whole of the success path, since the commit already removed each
claim as it wrote. A non-zero mask names the at-most-three keys still standing,
and each is removed from its read map directly. Nothing is searched in either
direction — not the binding tables by producer, and not the store by name.

That index addresses a *statement*, not a slot, and the two are not one-to-one: a
statement's eagerly-dispatched sub-slots share its lexical chain, so the index
alone does not say whose claims sit at it. The stamping slot is therefore marked
as it stamps — `own_claim_edges` on the
[`SlotFrame`](../../src/machine/execute/nodes.rs) records claim ownership beside
the edges — and the retirement hook consults the store only for a slot that
carries the mark. A sub-slot of a still-live binder retires its own edges and
leaves its statement's claims standing.

Two properties make that indexing sound, and both are worth asserting rather than
assuming. **A claim-owning slot never tail-replaces**, so the scope its claims
were installed into is the scope it retires against:
[`block_tail`](../../src/machine/core/kfunction/block_tail.rs) is the only
`Action::Tail` constructor and its callers are `MATCH` / `TRY` arms, `EVAL`, and
`USING`, none of which is a binder form in
[`BINDER_SPECS`](../../src/machine/model/binder.rs) — an FN body's tail belongs
to the call's slot, not the declaration's. And **a scope is fanned out into
exactly once**, which is what lets `by_statement` be a fixed run sized at the
fan-out.

What retirement really catches is the binder body that failed before its write
path: its name was never introduced, so a sibling that had parked on the claim
re-decides on wake against a scope where the name is absent and surfaces
`UnboundName` rather than the binder's own failure. That is the cost of not
leaving a claim behind for a *later* sibling to park on and never be woken by.

### Claims are for backward references, not forward ones

A claim never lets a reference reach a *later* statement. Visibility is the
strict `idx < cutoff` gate and it filters claims exactly as it filters bindings,
so a later-positioned binder is invisible whether or not it is in flight. What a
claim buys is the *backward* reference to an earlier sibling that has not
finished: a block's statements evaluate concurrently, so statement 3 may name
`x` from statement 1 while statement 1 is still running, and without a claim that
lookup would race between `UnboundName` and the bound value.

The one cross-order type-name resolution that survives the lexical gate is not a
claim at all: a module body's top-level type declarations are announced before
any of them runs, as an
[`AnnouncedWindow`](../../src/machine/model/types/declaration_window.rs) carried on the
child [`Scope`](../../src/machine/core/scope.rs) rather than in `Bindings`
(see [typing/elaboration.md](../typing/elaboration.md)). Mutual recursion is that
window's business, and it never reaches the claim store.

A driver that submits one statement at a time and runs each to completion
therefore consults no claim at all, because every visible binder has already
committed. So the statement-at-a-time door runs no fan-out: it sizes no statement
run and rules on no duplicate name, and a binder submitted through it stamps and
retires its claim without any reader ever seeing it. Sizing `by_statement` at the
fan-out is sound precisely because a fanned-into scope is fanned into exactly
once — the assertion above — and a claim arriving outside a fan-out grows the run
to reach its own index.

### Miri forward-splice and dispatch-park lifetime contract

A bare-name slot whose name resolves to a still-running producer is spliced out
as an alias of it (see [Bare-name forward splice](scheduler.md#bare-name-forward-splice)). A
read of the aliased slot resolves to the producer and returns the producer's own
`&KObject<'a>` reference — not a clone. The producer's region therefore must
outlive every consumer that reads through the alias. The dispatch-park route is
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
[`KoanRuntime::submit_expression`](../../src/machine/execute/decide/submit.rs)
reads that plan **once**, for a statement submission, and stamps its claims — a
`by_name` entry for the name channel, and a `by_bucket` entry per bucket key,
recorded together in that statement's `by_statement` slot — on the dispatching
scope, at `BindingIndex::value(chain.index)` and before the slot is ever popped
from the work queues. Each channel gets its **own** edge, wired from the freshly
allocated slot toward the dispatching scope's region: the submission holds that
region's owner across the install, which is the wiring-time proof the
destination is pinned, and the slot takes ownership of every edge it stamped.
Wiring before the slot can run is also why the install is always *parked* — a
claim can never come back filled. A later sibling that dispatches before the
statement's slot pops finds the entry and parks rather than surfacing
`UnboundName` / `DispatchFailed`. There is exactly one install site, at statement
submission; nothing installs at dispatch/pick time. The binder logic lives in the
dispatch layer, not the scheduler: the scheduler exposes only a generic slot
allocator (`Scheduler::alloc_node`), the install door, and the `Scope::install_*`
primitives, so no `NodeWork` variant or scheduler code names a `KExpression`.

A statement's plan is its own spine and nothing else, so the namespace a block
introduces is legible from its statement keys alone — which is what
order-independent sibling submission needs. A combined form stamps both channels
from that one slot at one `BindingIndex` — two edges, one owner — so a sibling
parked on the name and a sibling parked on the bucket wake on the same statement
and their claims retire together.

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
[`KoanRuntime::enter_block`](../../src/machine/execute/harness.rs) mints
chain frames at indices `1..N` for the N statements it submits. A
statement-at-a-time driver — a REPL session, the test harness's cursor —
declares the position itself: the submission doors
([`dispatch_in_scope`](../../src/machine/execute/harness.rs), `add`,
`add_dep_finish`) take an explicit statement `index`, placing the node
exactly as if it were the `index`-th line of a file. Only the driver knows
the position, so it is a parameter, never stored or derived. Every
submission therefore carries a real lexical chain, and the exclusive
cutoff enforces lexical well-foundedness universally: a statement's own
claims are hidden from its whole subtree at any depth, and a forward
reference to a lexically later top-level statement is a resolution error,
not a park.
