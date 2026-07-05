# Region-hosted witness sets

This doc owns the **representation and ownership model of witness sets**: where
a reach set lives, what a carrier holds instead of owning its set, and the
single pinning invariant that makes both sound.
[scheduler-library.md](scheduler-library.md) owns the library/embedder boundary
the types sit on; [per-node-memory.md](per-node-memory.md) owns the carrier
construction and access mechanics this representation slots under. Type names
here are working names — shapes are the commitment, identifiers are not.

Terminology bridge: the **witness set** here is the *reach set* of
[scheduler-library.md's vocabulary](scheduler-library.md#vocabulary) — the set
of regions a stored value's borrows can reach — used as the value's liveness
witness. One term, two roles: naming the reach and pinning it.

## The shape

- Every region's storage bundle carries a **witness-set sub-arena**: witness
  sets are a [`Stored`](../workgraph/src/witnessed/region.rs) family in the
  embedder's storage profile, allocated through the same single store path as
  every value family. No parallel storage engine exists for them.
- A witness set is **per-object and precise**: the exact home-omitted foreign
  reach of one stored value. Hosting fixes where the set's bytes live; it does
  not change granularity. There is no whole-region merged set — two values in
  one region with different reaches reference two different sets.
- A witness set is **frozen at store**. No site mutates a stored set;
  composition mints a new set into a destination arena. This is load-bearing,
  not style: carriers share references to one set, so growing a shared set
  silently widens every sharing carrier's witness (over-pinning — safe
  direction, semantically wrong), and shrinking one removes a pin out from
  under a carrier that relies on it (unsound).
- Sets **free at region death**, never individually. Every member `Rc`
  decrement a region's sets carry happens at that region's teardown — refcount
  traffic is batched at the frame level, the level at which drops happen.

## The two carrier forms

A carrier holds a reference to its witness set. Which reference depends on
where the carrier itself lives, and the two forms have different soundness
arguments. Getting the form wrong is the failure mode of this design; the
rules below are exhaustive.

**Resident form** — a carrier stored *inside* a region (a binding entry, any
in-region cell). It holds a bare `&'a WitnessSet` alongside its `&'a` value.

- *Soundness:* the reference is covered by the container's liveness. The entry
  is reachable only through its region (scope → frame → region), and the arena
  outlives every `'a` the region hands out — the same external-witness
  discipline as the `&'a KObject` stored beside it.
- *Rule (resident locality):* a resident set reference points only into **its
  own region's** arena. A reference into a foreign region's arena is not
  covered by the container's liveness and is never created. Consequence:
  binding a delivered carrier into a scope copies the delivered set's members
  into a set minted in the binding's home arena — one mint per bind, and every
  later read of that entry is a plain reference copy.
- *Cycle-safety:* the resident form stores no `Rc` in the entry at all, so a
  binding cannot close a `frame → region → scope → bindings → frame` strong
  cycle. Home-omission does the rest (below).

**Walking form** — a carrier *outside* every region: a node slot's sealed
terminal, a dep crossing steps. It owns **one `Rc` of its host region's
frame-owner** plus the lifetime-erased set reference.

- *Why bare fails here:* sets are home-omitted — a set hosted in region A never
  contains A's own `Rc` (the arena would own a set that owns the arena's
  region: a strong self-cycle, and A never drops). With the host omitted,
  nothing in a bare reference pins A, and the reference dangles the moment A
  dies. The owned host `Rc` is therefore not optional for any carrier that can
  outlive a step.
- *Pin chain, stated once:* the host `Rc` keeps the frame storage at a fixed
  heap address (`Rc` is `StableDeref`) → the region's arenas, including every
  witness set stored in them, stay live → the referenced set's member `Rc`s
  stay held → every member region stays live and fixed-address. A walking
  carrier's effective pin is **{host} ∪ set members**.
- *The frameless case:* a run-region value needs no held pin (its backing
  outlives the carrier); its walking witness is the pins-nothing value — no
  host, no set. This is the witness type's `Default`.
- *Packaging:* the walking witness is one library type (working name
  `HostedWitness<F>`): host `Rc<F>` + erased set reference, or the frameless
  value. The scheduler's terminal slots store it directly; the workload
  supplies only the frame-owner type `F`. The host `Rc` *is* the producer
  owner, so consumer-pull needs no singleton-recovery accessor and no
  single-to-set widening lift.

The set reference itself rides the same erase/reattach substrate as values: it
is stored lifetime-erased and re-anchored only under a live pin (the host
borrow for the walking form, the container borrow for the resident form).
Reading a set's members is always a pinned read.

## Composition: minting a set

Every union — a merge of two carriers, a bind fold, a finalize reseal, a
transfer between regions — **mints a new frozen set into a destination
region's arena**. The mint verbs take a destination allocation capability;
every site that composes witnesses has one in hand:

- a step finish: the consumer's region, held by the scheduler for the step
  (guarantee 4 of [scheduler-library.md](scheduler-library.md#the-guarantees));
- a scope bind or adoption: the scope's region owner, upgraded at the bind;
- finalize/close: the producer frame being folded in.

The mint reads its source sets' members **precisely** — whether the sources are
walking or resident, the member list is exact, so a value's witness never
coarsens to "everything its host region reaches." It then applies, against the
destination:

1. **Home-omission** — the destination's own region is never a member of a set
   hosted in it (the self-cycle rule above).
2. **Outer-chain subsumption** — a member whose region another member's
   `pins_region` owner chain already keeps alive is dropped, so the set stays
   an antichain of deepest owners. The subsumption hook is the embedder's
   [`PinsRegion`](../workgraph/src/witnessed/region_set.rs) impl, unchanged in
   role: mechanism library-owned, member semantics workload-supplied.

Acyclicity of the region graph is what keeps in-region strong `Rc`s from
leaking, and it rests on exactly two rules: home-omission (no self edge) and
the per-call frame rule that a dispatched frame strong-owns no lexical
ancestor ([per-call-region/](per-call-region/README.md)). The region engine's
no-allocation-back-edge property is scoped accordingly: a stored value holds
no owning `Rc` back to **its own** region; owning `Rc`s to *foreign* regions
are exactly what a hosted witness set is for.

## The pinning invariant

The one contract every unsafe re-anchor in the system discharges against,
numbered for implementors:

1. A stored witness set's members are strong frame-owner `Rc`s; the set lives
   exactly as long as its hosting region.
2. Holding a region's owner `Rc` pins every witness set in that region's arena
   and, through their members, every region those sets name.
3. A **resident** carrier's pin is its container region's liveness (external —
   the reader's borrow of the region is the witness). A **walking** carrier's
   pin is its owned host `Rc` (internal — the bundle is the witness).
4. A **deposit** — a set minted into a region's arena with no accompanying
   stored value — is legal and is the ownership slot for adoption-style
   re-anchors: a foreign carrier adopted for in-region use at `'a` deposits its
   member list into the home arena, and the deposit persists until region
   death, which bounds every `'a` the region hands out. There is no other
   pinning channel: every pin in the system is a hosted set, region-owned.

## Scope and bindings above the substrate

The Koan layers compose the substrate; they hold no witness state of their
own.

- **The scope keeps no reach accumulator.** Its two jobs are covered by the
  arena directly:
  - *Pin-deposit:* adopting a sealed dep for in-scope use mints a deposit
    (invariant 4) in the home arena. The re-anchor's SAFETY argument is the
    region-death bound — uniform with every other pin — not a scope-held set.
  - *Aggregate read:* a module's foreign reach is the union over its child
    scope's **binding entries'** sets, minted once at scope close (the seal
    point). Deposits are arena entries, not binding entries, so transient
    adoptions are excluded by construction — a module's reach names only what
    its members reach.
- **Binding entries** store the erased value, a bare resident set reference,
  and the binding index. Reads copy the thin reference; no entry owns a set,
  and no lookup clones one. This also removes the reach column from the
  member/type/memo read payloads — the reference is the payload.
- **Omission policy stays scope-derived.** Which regions a bind may omit (the
  home frame, lexical-ancestor regions the chain already pins) is computed by
  the scope (`chain_reaches_region`) and passed to the mint as a predicate;
  the mint mechanism itself is library code.
- **Optional footprint index.** A per-region "already pinned" summary can make
  a fully-covered deposit a no-op, bounding deposits at O(distinct regions)
  instead of O(adoptions). It is pure footprint optimization — invariant 4
  holds with or without it — and is explicitly not soundness-bearing.

## Retention model

A set pins its members until its hosting region dies; a carrier's death
releases only its host `Rc`. The bounds:

- a **per-call region** frees at call end — deposits and terminal sets pin for
  at most the call;
- a **TCO-reused frame** frees its storage each tail iteration — one
  iteration's sets never outlive the iteration;
- the **run region** is the residual: a set minted into the run arena pins its
  members for the program. The lever that keeps this small is precision at the
  mint: a region-pure value (a scalar, a deep copy that produced no region
  borrow) mints the **empty** set and pins nothing.

This trade is deliberate: member releases batch at region teardown instead of
scattering across carrier lifetimes.

## Library boundary

Per the [scheduler-library.md](scheduler-library.md) division:

- **Library-owned:** the witness-set type and its mechanism (freeze-at-store,
  home-omission, subsumption fold), the hosting family plumbing, the walking
  witness type (`HostedWitness`), and the mint verbs (which require a
  destination allocation capability by signature).
- **Workload-supplied:** the frame-owner type `F` with its `PinsRegion`
  subsumption hook, the storage profile listing the witness-set family, and
  the omission-policy predicate at bind sites.

## Open work

- [roadmap/scheduler_library/region-hosted-witness-sets.md](../roadmap/scheduler_library/region-hosted-witness-sets.md)
  — the implementation item: hosting family, carrier forms, scope/bindings
  migration, and the library-surface consolidation.
- [roadmap/scheduler_library/region-pure-empty-reach.md](../roadmap/scheduler_library/region-pure-empty-reach.md)
  — the retention model's precision lever: fully-owned values mint the empty
  set at the fold points instead of inheriting producer/dep reaches.
