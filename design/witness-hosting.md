# Region-hosted witness sets

This doc owns the **representation and ownership model of witness sets**: where a
reach set lives, how a carrier references it, and what keeps that reference
pinned. [scheduler-library.md](scheduler-library.md) owns the library/embedder
boundary the types sit on; [per-node-memory.md](per-node-memory.md) owns the
carrier construction and access mechanics this representation slots under. Type
names here are working names — shapes are the commitment, identifiers are not.

Terminology bridge: the **witness set** here is the *reach set* of
[scheduler-library.md's vocabulary](scheduler-library.md#vocabulary) — the set of
regions a stored value's borrows can reach — used as the value's liveness witness.
One term, two roles: naming the reach and pinning it.

## The shape

- Every region's storage bundle carries a **witness-set sub-arena**: witness sets
  are a [`Stored`](../workgraph/src/witnessed/region.rs) family in the embedder's
  storage profile, allocated through the same single store path as every value
  family. No parallel storage engine exists for them.
- A witness set is **per-object and precise**: the exact home-omitted foreign
  reach of one stored value. Hosting fixes where the set's bytes live; it does not
  change granularity. There is no whole-region merged set — two values in one
  region with different reaches reference two different sets.
- A witness set is **frozen at store**. No site mutates a stored set; composition
  mints a new set into a destination arena. This is load-bearing: carriers share
  references to one set, so growing a shared set would silently widen every
  sharing carrier's witness (over-pinning — safe direction, semantically wrong),
  and shrinking one would remove a pin out from under a carrier that relies on it
  (unsound).
- Sets **free at region death**, never individually. Every member `Rc` decrement a
  region's sets carry happens at that region's teardown — refcount traffic batched
  at the frame level, the level at which drops happen.

## The carrier

There is **one** carrier witness, the same whether the carrier is stored inside a
region or in flight between nodes:

```
{ borrows_host: bool, reach: &WitnessSet }
```

a reference to the value's frozen reach set — hosted in the value's **host
region's** own arena — plus one bit. The carrier **owns no `Rc`** and never owns
its set.

- *Home-omission and the bit.* A set hosted in region A can never contain A's own
  `Rc` (the arena would own a set that owns its own region: a strong self-cycle,
  and A never drops). So "the value borrows into the region it lives in" cannot
  ride as a set member; it rides as `borrows_host`. The set names only the value's
  **foreign** reach.
- *Empty reach is `None`, not a hosted empty set.* A region-pure value's reach set
  is encoded as `None` — the mint verb (§ Composition) skips the store entirely
  when the composed set is empty, so a region-pure bind allocates nothing. `None`
  **is** the empty set; it is not a missing value, and every reader treats it as
  "pins nothing" rather than "not yet computed."
- *Reference-only, liveness external.* Because the carrier holds no pin, what keeps
  its `&WitnessSet` valid comes from outside it — and the source differs by where
  the carrier sits, while the representation does not:
  - **Resident** (a binding entry, any in-region cell): the container's liveness
    covers the reference. The entry is reachable only through its region (scope →
    frame → region), and the arena outlives every `'a` the region hands out — the
    same external-witness discipline as the `&'a` value stored beside it.
  - **Walking** (a node slot's sealed terminal, a dep crossing steps): the
    scheduler **retains the value's host frame until every destination has pulled
    the terminal** (§ Retention model). The host arena — hence the referenced set
    — stays live and fixed-address for the terminal's whole dwell.
  One `{ bit, ref }` shape covers both. There is no owned host `Rc`, no
  severed-backing arm, and no separate liveness-pin channel beside the reach: a
  value's host is kept alive by retention or containment, its foreign reach by the
  set members. A walking value travels as a **delivery envelope**
  ([`Delivered`](../workgraph/src/witnessed/delivered.rs)) — the sealed carrier
  paired with the retained host frame `Rc` — so the retention hold that pins the
  carrier's set is carried alongside it to every consumer, and the carrier itself
  stays pin-free.
- *Resident locality.* A carrier's set reference points only into its **host
  region's own** arena. A reference into a foreign region's arena is never created
  — it would not be covered by the container's or the retention's liveness.
- *Cycle-safety.* The carrier stores no `Rc`, so a binding cannot close a `frame →
  region → scope → bindings → frame` strong cycle. Home-omission does the rest.

`borrows_host` is a **reach-representation** bit only. It is consumed at exactly
one moment — when the value's reach is minted into a *different* destination arena
(§ Composition) — and it never influences when a frame is released (§ Retention
model). Nothing about a carrier ever "severs" a frame.

The set reference rides the same erase/reattach substrate as values: stored
lifetime-erased, re-anchored only under a live pin (the container borrow when
resident, the retained host frame when walking). Reading a set's members is always
a pinned read.

## Composition: minting a set

Every union — a merge of two carriers, a bind fold, a finalize reseal, a transfer
between regions — **mints a new frozen set into a destination region's arena**.
The mint verbs take a destination allocation capability; every composing site has
one in hand:

- a step finish: the consumer's region, held by the scheduler for the step
  (guarantee 4 of [scheduler-library.md](scheduler-library.md#the-guarantees));
- a scope bind or adoption: the scope's region owner, upgraded at the bind;
- finalize/close: the frame being folded in.

The mint reads its source sets' members **precisely** — the member list is exact,
so a value's witness never coarsens to "everything its host region reaches." It
then applies, against the destination:

1. **Home-omission** — the destination's own region is never a member of a set
   hosted in it (the self-cycle rule).
2. **Borrows-host materialization** — if a source carrier's `borrows_host` is set
   and its old host region is **foreign** to the destination, that old host becomes
   a concrete member of the minted set. A value's own home is a bit while the value
   stays home; it becomes a named member the moment the value is re-homed
   elsewhere.
3. **Outer-chain subsumption** — a member whose region another member's
   `pins_region` owner chain already keeps alive is dropped, so the set stays an
   antichain of deepest owners. The subsumption hook is the embedder's
   [`PinsRegion`](../workgraph/src/witnessed/region_set.rs) impl: mechanism
   library-owned, member semantics workload-supplied.

A **pure pass-through** — a value returned up the call stack unmodified — runs no
mint: its carrier rides by reference, host unchanged, so a closure handed up N
frames costs zero allocations and zero refcount traffic. A mint runs only where a
value is genuinely **re-homed** into a longer-lived region.

Acyclicity of the region graph is what keeps in-region strong `Rc`s from leaking,
and it rests on two rules: home-omission (no self edge) and the per-call frame
rule that a dispatched frame strong-owns no lexical ancestor
([per-call-region/](per-call-region/README.md)). A stored value holds no owning
`Rc` back to **its own** region; owning `Rc`s to *foreign* regions are exactly what
a hosted witness set carries.

## The pinning invariant

The one contract every unsafe re-anchor in the system discharges against, numbered
for implementors:

1. A stored witness set's members are strong frame-owner `Rc`s; the set lives
   exactly as long as its hosting region.
2. Holding a region's owner `Rc` pins every witness set in that region's arena and,
   through their members, every region those sets name.
3. A carrier holds no pin of its own. Its `&WitnessSet` is covered externally: by
   the **container's** liveness when the carrier is resident, by the scheduler's
   **frame-retention** when it is walking.
4. **Frame-retention.** The scheduler holds a producer frame's owner `Rc` until
   every destination has pulled its terminal; the frame is released when that
   pull-count reaches zero. Release is a function of deliveries only — never of any
   value's reach.

## Scope and bindings above the substrate

The Koan layers compose the substrate; they hold no witness state of their own.

- **The scope keeps no reach accumulator and no deposit list.** It stores the
  hosted carrier directly. Binding a delivered carrier into a scope **mints** its
  reach into the scope's home arena (§ Composition), producing a resident
  `{ bit, ref }` entry whose set members pin the foreign regions for the scope's
  life — the job a reach accumulator and a deposit list would otherwise split,
  folded into the one resident set.
- **Binding entries** store the erased value, the resident set reference, and the
  `borrows_host` bit. Reads copy the thin reference; no entry owns a set, and no
  lookup clones one.
- **Module reach** is the union over the child scope's **binding entries'** sets,
  minted once at scope close (the seal point).
- **Omission policy stays scope-derived.** Which regions a bind may home-omit (the
  home frame's storage pin chain, lexical-ancestor regions the chain already pins)
  is computed by the scope (`Scope::covers_region_ambiently`) and passed to the
  mint as a predicate; the mint mechanism itself is library code. The same
  predicate is the evidence-tier residence audits' ambient coverage, so what a
  mint omits an audit still accepts.

## Retention model

A set pins its members until its hosting region dies; those releases batch at
region teardown. A carrier's own death releases nothing — it owns nothing.

The lifetime of a **host frame** is the scheduler's frame-retention: it lives until
every destination of its terminals has pulled. A walking terminal carries this hold
as its [`Delivered`](../workgraph/src/witnessed/delivered.rs) envelope's host `Rc`,
so the pin travels with the value to each consumer rather than riding the carrier.
Two consequences:

- A **pass-through** value — a closure returned up the stack unmodified — stays
  hosted in its **birth frame** and rides up by reference. That birth frame is
  retained across the whole return chain, and released only once the value is
  **re-homed** into a longer-lived region (a bind, where the mint materializes the
  old home into the destination set) or dropped. Ordinary returns are zero-copy;
  the re-home is the only place members move.
- The **run region** is the residual: a set minted into the run arena pins its
  members for the program. The lever that keeps this small is precision at the mint
  — a region-pure value mints the **empty** set and pins nothing.

**TCO** consumes retention directly. A tail call reinstalls the slot's work,
keeping its node identity; the retiring incarnation's region — hosting the
arguments it sealed as carriers — is held by retention until the reinstalled
incarnation adopts them (release at pull-count zero), so the region free is ordered
after the adoption copy. Koan issues only the reinstall and never touches a region.
The full design is [tail-call-optimization.md](tail-call-optimization.md).

## Library boundary

Per the [scheduler-library.md](scheduler-library.md) division:

- **Library-owned:** the witness-set type and its mechanism (freeze-at-store,
  home-omission, borrows-host materialization, subsumption fold), the hosting
  family plumbing, the
  [`{ borrows_host, reach }` carrier](../workgraph/src/witnessed/carrier.rs), the
  mint verbs (which require a destination allocation capability by signature), and
  the scheduler's frame-retention (release at pull-count zero).
- **Workload-supplied:** the frame-owner type `F` with its `PinsRegion` subsumption
  hook, the storage profile listing the witness-set family, and the omission-policy
  predicate at bind sites.
