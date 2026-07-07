# Tail-call optimization

This doc owns the **TCO design as a whole**: why a tail call runs in constant
space, how it does so purely in the library's node vocabulary, and the soundness
argument that region turnover never frees memory still in use. TCO is a Koan-side
concern expressed as a single node operation — Koan reinstalls a slot's work and
never manipulates a region. Region lifetime is the library's, tied to node
lifetime. This doc is the reasoning layer over [witness-hosting.md](witness-hosting.md),
which owns the reference-only carrier and the frame-retention model the free
ordering rests on, and [scheduler-library.md](scheduler-library.md), which owns the
library/Koan boundary this design honors.

## What a tail call must not cost

A tail call is the last thing a body does: its result is its caller's result, with
nothing left to observe the callee afterward. A naive implementation still pays for
the call — a scheduler node to run the callee, and a per-call region to hold its
allocations. Down a recursion `N` deep that is `O(N)` nodes and `O(N)` regions: a
loop written as recursion exhausts the node table and the heap.

TCO removes both costs. Deep tail recursion is **constant-memory** — in the node
table and on the heap alike — so recursion is a sound substitute for iteration.

## The design: reinstall the slot, turn over the region

A tail call is one node operation: the slot the caller ran is **reinstalled** with
the callee's work
([`reinstall`](../workgraph/src/scheduler/node_store.rs)), keeping its node
identity. Because the identity is stable, the consumer awaiting the slot's result
is untouched — no forward, no consumer re-point, no alias tombstone. Koan issues
only the reinstall; the library reacts to it by **turning over the region**: the
node's previous incarnation held a region, and the reinstalled incarnation gets a
fresh one. The old region is not Koan's to drop — it is handed to retention (§
Region liveness) and freed once the new incarnation has adopted the carried
arguments.

Constant space follows: one slot serves the whole loop (`O(1)` node table), and at
steady state at most two regions are live — the retiring incarnation's, still
pinned while its arguments are adopted, and the running one. A depth-`N` loop is
`O(1)` in both.

Because the reinstall applies **after** the caller's step returns
([apply_outcome](../src/machine/execute/runtime.rs) is the sole graph-writing site,
never mid-step), the old incarnation's region is past every borrow into it by the
time it is retired. No reserve frame, no in-place reset, no two-iteration timing:
the scheduler's ordinary run-then-apply ordering supplies the safety a synchronous
in-step reset could not.

## Region liveness by node lifetime

A node mints a per-call region only when its incarnation opens a new allocating
lexical scope (a FN-body invoke, a MATCH/TRY arm). The mint is **lazy** — the
library creates an incarnation's region on its first allocation, not at reinstall —
so an incarnation that opens no such scope mints nothing:

- **Syntactic reductions** — a `(inner)` parenthesization or `:(...)` type-context
  unwrap re-dispatches the inner expression, inheriting the ambient scope.
- **Bare-name forwards** — a slot that is just a name binding to an existing
  producer is spliced out; it holds no value of its own.
- **`USING` overlay entries** — the body runs in a caller-allocated overlay scope,
  owning no region.
- **Top-level / run producers** — top-level statements adopt the non-dying run
  region, which outlives the program.

An allocation-light tail hop therefore mints **no** region at all; only a hop that
genuinely allocates pays for one.

`CallFrame` is a *reference* to its incarnation's library-owned region, not the
region's owner — it names allocation and scope for the running step and holds no
power to create or destroy a region. See [per-call-region/](per-call-region/README.md)
for the frame's scope-handle and lexical-chain role.

## Loop-carried values

A tail call carries values forward — its arguments, any closure it re-invokes.
These are the reinstalled incarnation's **owned deps**: sealed as carriers in the
retiring incarnation's region during its step, then adopted into the new
incarnation's fresh region at its first step (`transfer_into`, the ordinary
carrier-delivery path — see
[scheduler-library.md § The consumer API](scheduler-library.md#the-consumer-api)).
Argument passing is not a TCO special case; it is the same one structural copy per
value that any dep crossing a region boundary pays. The adoption is also what times
the retiring region's free: it lives until the new incarnation has adopted (§
Soundness, Lemma 2).

## Space and efficiency

| Cost | Naive tail call | TCO |
| --- | --- | --- |
| Nodes for a depth-`N` loop | `O(N)` | `O(1)` — one slot, reinstalled |
| Live per-call regions (steady state) | `O(N)` | `O(1)` (≤ 2 transiently) |
| Region mint per hop | 1 | 0 if the hop allocates nothing (lazy mint), else 1 |
| Loop-carried argument | copy | copy — ordinary dep adoption |
| Pure pass-through (value returned unmodified) | per-node | zero — carrier rides by reference |

A value returned up the stack unmodified is a pure pass-through: its carrier rides
by reference, host unchanged, costing no set allocation and no refcount traffic
([witness-hosting.md § Composition](witness-hosting.md#composition-minting-a-set)).
A mint runs only where a value is genuinely re-homed into a longer-lived region —
the loop-carried argument adoption, and the loop's final escaping result. An
allocation-light loop is strictly cheaper than a naive call: `O(1)` everything and
zero region mints per hop.

## Soundness

The design is sound iff region turnover never frees memory a live borrow or a
pending adoption still needs. Three lemmas, each resting on a checked mechanism.

**Lemma 1 — a region is freed only after its incarnation's step returns.** The
reinstall applies after the caller's step returns, so by the time the retiring
incarnation's region is handed to retention every tree-borrows protector into it is
released. Freeing under a live borrow is therefore impossible. *Enforced by* the
run loop's ordering: graph edits apply after a step returns, never mid-step.

**Lemma 2 — the retiring region outlives argument adoption.** The retiring
incarnation seals its carried arguments as carriers hosted in its own region; the
reinstalled incarnation adopts them into its fresh region on its first step.
Frame-retention holds the retiring region's owner until every destination has
pulled — here, until the one successor incarnation adopts (release at pull-count
zero, [witness-hosting.md § Retention model](witness-hosting.md#retention-model)).
So the free is ordered *after* the adoption copy, never before, and the single
consumer of tail position makes the release prompt.

**Lemma 3 — cross-region references are witnessed, never raw.** An incarnation
whose lexical chain reaches into an enclosing frame's region (a MATCH/TRY arm
resolving free names against its surrounding call) holds that reach as a
**witness-set member** pinning the enclosing region, not a raw pointer — the
reference-only carrier model
([witness-hosting.md § The carrier](witness-hosting.md#the-carrier)). So no
reference outlives the region it names: the enclosing region is retained exactly as
long as an incarnation's reach names it. *Enforced by* the carrier being the only
way to hold a cross-region borrow.

Together: Lemma 1 rules out freeing under a live borrow, Lemma 2 rules out freeing a
region an adoption still reads, Lemma 3 rules out a raw cross-region pointer. Stable
node identity removes the fourth failure mode a fresh-id design would carry — no
consumer edge is ever re-pointed, so none can name a freed slot. Region turnover
frees exactly the region nothing else holds.

### Interaction with bare-name forwarding

A loop's result can be consumed by a bare-name forward — `let x = <loop>` where
another node references `x`. That reference resolves to the loop node and, while the
loop is still iterating, parks: the loop node is not `is_result_ready` until it
finalizes (intermediate hops reinstall it, never finalize it), so the forward takes
the `Alias` path and `splice_forward` moves its consumers onto the loop node's
notify list. A tail hop reinstalls the slot **without touching its notify list**, so
those consumers survive every iteration and fire on the final terminal, resolving
their alias to the loop node's stable id. **This composes only because node identity
is stable**: an alias taken mid-loop names the same slot at loop exit. A fresh-id
design would strand it.

The loop's **final** result may therefore fan out to several consumers (the binding
plus any forwards); retention holds the final incarnation's region until every one
has pulled. This is a different quantity from Lemma 2's retiring→successor handoff,
which is always exactly one consumer — the fan-out is on the loop's terminal, not on
the intra-loop argument adoption.

### The kept-first return contract

A tail chain checks the value against the **first** caller's declared return, not
the tail-most callee's, so the reinstalled slot keeps the first contract it entered.
The contract's declared type points into an *ancestor* region — for a FN/per-call
contract the callee's captured scope (pinned by the closure value independently of
the loop), for a MATCH/TRY arm the call-site region. That home region is named by
the reinstalled node's **reach set**, a witnessed cross-region borrow (a corollary
of Lemma 3): retention keeps it alive across every hop, and the keep-first rule
retains the first contract's reach alongside the contract. Nothing pins it through
the per-call region the loop turns over, so the contract survives the reinstall
without a raw ancestor-frame reference.

## Library boundary

Per [scheduler-library.md](scheduler-library.md): the library owns regions
wholesale — the arena engine, the witness-set sub-arena, the allocation capability,
and the delivery-driven frame-retention. Koan owns only the *node operation*: it
reinstalls a slot. Region lifetime falls out of node lifetime — an incarnation's
region is minted lazily on first allocation and retired when the slot is
reinstalled (or the node freed) — so Koan issues no region mint, reset, or drop,
and the embedder boundary stays clean by construction rather than by convention.
