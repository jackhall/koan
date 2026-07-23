# Witness sets: reach descriptions and pin bundles

This doc owns the **representation and ownership model of witness sets**: how a
value's reach is described, what keeps the regions it reaches alive, and which
holder owns that keeping-alive. [scheduler-library.md](scheduler-library.md)
owns the library/embedder boundary the types sit on;
[per-node-memory.md](per-node-memory.md) owns the carrier construction and
access mechanics this representation slots under. Type names here are working
names — shapes are the commitment, identifiers are not.

Terminology bridge: the **witness set** here is the *reach set* of
[scheduler-library.md's vocabulary](scheduler-library.md#vocabulary) — the set
of regions a stored value's borrows can reach — used as the value's liveness
witness.

## The split: two types, two jobs

Reach evidence is two separate types, and nothing in the system conflates them:

- The **reach description** answers *questions* — "which foreign regions does
  this value borrow into?" (`pins_region`, membership queries). It is
  non-owning and `Copy`-cheap to reference: holding a description keeps
  **nothing** alive. It is pure data.
- The **pin bundle** answers *liveness* — an owned collection of strong
  frame-owner `Rc<FrameStorage>`s (the value's host frame plus every foreign
  region its reach names). Holding a bundle is what keeps regions alive.
  Dropping it is what releases them.

The two are minted **together, as an inseparable pair**, by the derivation
doors (§ Composition). There is no constructor that builds a description from
loose parts, and no constructor that builds a bundle except alongside the
description it covers. A holder that has only a description has no way to
re-anchor the value it describes — the re-anchor doors require the bundle by
signature. This is the compile-safety line: *using a description where
ownership is required does not typecheck.*

## The description

- Descriptions live in an **append-stable side table owned by the region's
  `FrameStorage`** — ordinary heap data, dropped when the region drops. They
  are **not** arena-page data, so arena pages carry no `Drop`-bearing reach
  state ([value-substrates.md](value-substrates.md)'s untyped `Drop`-free end
  state needs the pages clean). The table is append-only in address: a
  description's `&` stays valid for the region's whole life, which is what
  lets carriers share thin references to it.
- A description is **per-object and precise**: the exact home-omitted foreign
  reach of one stored value. There is no whole-region merged description — two
  values in one region with different reaches reference two different entries.
- A description is **frozen at mint**. No site mutates one; composition mints
  a new entry. This is load-bearing: carriers share references to one
  description, so growing a shared one would silently widen every sharing
  carrier's claimed reach, and shrinking one would falsify a claim some
  carrier still relies on.
- The description is **not a storage family**: it is not allocated through the
  value store path and carries no `Stored`/`Reattachable` bounds. Only values
  live in arenas; reach metadata lives beside them, in the table.
- **Empty reach is `None`, not a hosted empty entry.** A region-pure value's
  description is `None` and its bundle is empty — a region-pure bind allocates
  nothing and refcounts nothing. `None` **is** the empty set; every reader
  treats it as "reaches nothing," never "not yet computed."

## The carrier

There is **one** carrier witness, the same whether the carrier is stored
inside a region or in flight between nodes
([`Carrier`](../workgraph/src/witnessed/carrier.rs)):

```
{ borrows_host: bool, reach: &ReachDescription }
```

one bit plus a reference into the value's **host region's** side table. The
carrier owns nothing — no `Rc`, no bundle — so it stays `Copy` and a binding
cannot close a `frame → region → scope → bindings → frame` strong cycle.

- *Home-omission and the bit.* A description names only the value's
  **foreign** reach; "the value borrows into the region it lives in" rides as
  `borrows_host`, never as a member. The reason is on the bundle side: a
  resident holder's bundle lives inside its own region (scope → bindings →
  entry), so a bundle member naming the holder's own region would make the
  region transitively own itself — a strong self-cycle that never drops. Home
  stays a bit while the value is home; it becomes a named member the moment
  the value's reach is minted into a *different* region (§ Composition).
- *Reference validity.* What keeps a carrier's `&ReachDescription` valid is
  the host region's liveness, and what keeps the host region alive is always
  some holder's **owned bundle** (or containment inside the region itself).
  The carrier never carries its own liveness; § The holder rule says who must.

The description reference rides the same erase/reattach substrate as values:
stored lifetime-erased, re-anchored only under a live pin. Reading a
description's members is always a pinned read.

## The pin bundle

The bundle is an owned value — working shape `Vec<Rc<FrameStorage>>` (the
concrete container is an identifier-level choice, not part of the shape
commitment). Its contents are: the value's **host** frame owner (when the holder is outside
that region) plus one owner per **foreign** region the description names.
Where bundles live:

- **The delivery envelope** ([`Delivered`](../workgraph/src/witnessed/delivered.rs)):
  a walking value — a node slot's sealed terminal, a dep crossing steps —
  travels as the sealed carrier paired with its full bundle, host included.
  Duplicating an envelope duplicates the bundle, so every fan-out consumer
  (staged-sub splices, catch continuations, spliced expression clones) holds
  its own pins for its own hold. The envelope is the *only* walking shape;
  a bare carrier never walks alone.
- **Binding entries**: a scope binding stores the erased value, the opaque
  reach token (`StoredReach` — description reference plus the `borrows_host`
  bit), **and the entry's owned bundle**, side by side. The entry's bundle is
  what makes the binding's pins real; the token is only the claim.
- **Transient pins**: short function-scope holds that re-anchor carriers —
  the run loop's per-step combined pin, the spliced-return check — hold
  explicit `Rc`s for exactly their scope. They never use a description as a
  pin: a description pins nothing.

## The holder rule (the pinning invariant)

The one contract every re-anchor in the system discharges against, numbered
for implementors:

1. A reach description keeps nothing alive. Ever. Any reasoning of the form
   "the description names region R, therefore R is alive" is wrong.
2. Regions are kept alive by owned bundles (and by a region's own internal
   containment). Holding a region's owner `Rc` keeps that region — its arena,
   its side table, its scope's binding entries — alive; those entries' own
   bundles keep *their* foreign reaches alive, recursively. Transitive
   coverage flows through **binding entries**, not through descriptions.
3. Every holder of a carrier that can re-anchor its value either **owns a
   bundle covering the carrier's full reach** (host + foreign members) for
   its whole hold, or is **enveloped**: it lives strictly inside the lifetime
   of another holder's bundle that covers the same reach (a within-step
   carrier under the step's pin, typed at the step's `'step` lifetime by
   [`StepCarried`](../src/machine/execute/step_carried.rs); an entry read
   under the entry's own bundle). "Enveloped" is a lifetime claim the borrow
   checker can see, not a convention.
4. The re-anchor doors take the bundle (or the enveloping borrow) by
   signature. A carrier plus no bundle is inert data.
5. Release is ordinary `Drop` of a bundle. There is no release verb, no
   un-mint, no audit. When a binding entry drops — rebind, evacuation, scope
   death — its pins drop with it.

## Composition: minting a pair

Every union — a merge of two carriers, a bind fold, a finalize reseal — mints
a **new frozen description into the destination region's side table and
returns it paired with the owned bundle**. The mint verbs take the
destination's allocation capability; every composing site has one in hand
(a step finish: the consumer's region, held by the scheduler for the step —
guarantee 4 of [scheduler-library.md](scheduler-library.md#the-guarantees);
a scope bind or adoption: the scope's region owner).

The mint reads its source descriptions **precisely** — a value's witness never
coarsens to "everything its host region reaches" — then applies, against the
destination:

1. **Home-omission** — the destination's own region is never a member of a
   description hosted in it, and never an `Rc` in a bundle stored resident in
   it (the self-ownership cycle, § The carrier).
2. **Borrows-host materialization** — if a source carrier's `borrows_host` is
   set and its host region is **foreign** to the destination, that host
   becomes a named member of the minted description and a strong `Rc` in the
   returned bundle.
3. **Outer-chain subsumption** — a member whose region another member's
   `pins_region` owner chain already keeps alive is dropped, so the pair stays
   an antichain of deepest owners. The subsumption hook is the embedder's
   [`PinsRegion`](../workgraph/src/witnessed/region_set.rs) impl.

A **pure pass-through** — a value returned up the call stack unmodified — runs
no mint: its carrier rides by reference inside its envelope, host unchanged,
so a closure handed up N frames costs zero mints and zero refcount traffic
beyond the envelope's travel. A mint runs only where a value's reach is
genuinely restated against a new region — a bind, an adoption, a merge.

Acyclicity of the region ownership graph rests on two rules: home-omission
(no self edge, in either the description or a resident bundle) and the
per-call frame rule that a dispatched frame strong-owns no lexical ancestor
([per-call-region/](per-call-region/README.md)).

## Escape: the single seam

A value escapes its producer frame in exactly one place: the **bind seam**,
where a consumer binds the delivered value into a scope. There is no second
escape channel.

- A **declared return** (an FN's `-> :T`, a MATCH/TRY arm's contract) is
  checked and re-stamped **in place**, in the producer's own region, at the
  Done boundary. The check moves no bytes and re-homes nothing. The sealed
  return obligation is pure `Copy` data — the declared type is a run-region
  registry handle ([typing/type-registry.md](typing/type-registry.md)) and
  the error label is precomputed at seal — so the obligation references no
  region, holds no pin, and carries no relocation destination. Under TCO the
  obligation rides the tail chain keep-first and the check fires once, at the
  chain's end, exactly as [tail-call-optimization.md](tail-call-optimization.md)
  schedules it.
- An **undeclared return** ends the same way: the value stays in its producer
  frame; the scheduler's retention hold (§ Retention) keeps that frame alive
  until every consumer pulls.
- At the bind seam the consumer prices **copy against pin**
  ([`copy_delivered_substrate`](../src/machine/core/scope/reach.rs), the cost
  model of
  [value-substrates.md § Cost-driven copy](value-substrates.md#cost-driven-copy-the-optimization)):
  *copy* rebuilds the value in the destination region and lets the producer
  frame free at retention discharge; *pin* stores the envelope's bundle in the
  binding entry, making the producer frame's region the value's residence for
  the binding's life. Both are always legal; the choice is pure cost.

Because a pin is entry-owned, a pinned residence ends when the entry does.
The canonical example, spelled out:

```
FN count : n = MATCH (n) (0 -> 0) (_ -> count : n - 1)
```

Each tail hop retires its frame per retention. If a loop-carried bind (`it`
in a WHILE-shaped tail loop) prices to **pin**, iteration N's entry holds
iteration N's producer region; iteration N+1's **rebind drops that entry**,
and the region frees. At most one retired region is ever live beyond the
current frame — pinning preserves the same O(1) region turnover as copying,
and the pricing may pick either.

Host = residence, by construction: a value is never moved out of its producer
region by any channel, so the envelope's host pin, the producer's retention
hold, and the value's residence region are one and the same region. An
envelope whose host does not pin its value's residence cannot be built.

## Retention model

The lifetime of a **host frame** is the scheduler's frame-retention: the
scheduler holds a producer frame's owner `Rc` until every destination of its
terminals has pulled; release is a function of deliveries only — never of any
value's reach, and `borrows_host` never influences it. A walking terminal
carries this hold inside its envelope bundle, so the pin travels with the
value to each consumer.

- A **pass-through** value stays hosted in its birth frame and rides up by
  reference; the birth frame is retained across the whole return chain and
  freed once the value is copied out or its last pinning entry drops.
- The **run region** is the residual: a bundle stored in a run-scope entry
  pins its members for the program. The lever that keeps this small is
  precision at the mint — a region-pure value's bundle is empty and pins
  nothing.
- **Region death** drops the region's side table and its scope's binding
  entries — and therefore every bundle those entries own. Refcount decrements
  for a region's outbound pins batch at that teardown; a rebind pays only its
  own entry's bundle.

**TCO** consumes retention directly: a tail call reinstalls the slot's work,
the retiring incarnation's region is held by retention until the reinstalled
incarnation adopts its sealed arguments, and the free is ordered after the
adoption. The full design is
[tail-call-optimization.md](tail-call-optimization.md).

## Scope and bindings above the substrate

The Koan layers compose the substrate; they hold no witness state of their
own.

- **The scope keeps no reach accumulator and no deposit list.** Binding a
  delivered value mints its reach pair into the scope's region and stores
  carrier, token, and bundle as one entry — the mint and the store are **one
  fused door** (`Scope::bind_delivered` / `bind_checked`, `bind_module`,
  `register_type_delivered` and siblings), so a scope entry cannot state a
  reach the value's borrows don't back, and cannot claim pins it doesn't own.
- **Reads stay refcount-free.** A binding read hands out the borrowed
  description enveloped under the entry's own bundle (holder rule 3); the
  bundle is cloned only when the value genuinely escapes to a new holder — a
  new envelope, a new entry.
- **Module reach** is the union over the child scope's binding entries' pairs,
  minted once at scope close; the parent entry owns the resulting bundle.
- **Omission policy stays scope-derived.** Which regions a bind may home-omit
  (the home frame's storage pin chain, lexical-ancestor regions that chain
  already pins) is computed by the scope (`Scope::covers_region_ambiently`)
  and passed to the mint as a predicate; the mint mechanism itself is library
  code.

## Library boundary

Per the [scheduler-library.md](scheduler-library.md) division:

- **Library-owned:** the description and bundle types and their mechanism
  (paired mint, freeze-at-mint, home-omission, borrows-host materialization,
  subsumption fold), the
  [`{ borrows_host, reach }` carrier](../workgraph/src/witnessed/carrier.rs),
  the envelope, the mint verbs (which require a destination allocation
  capability by signature), and the scheduler's frame-retention (release at
  pull-count zero).
- **Workload-supplied:** the frame-owner type `F` with its `PinsRegion`
  subsumption hook and the omission-policy predicate at bind sites.

## Open work

- [Reach ownership split and the single escape seam](../roadmap/untyped_arena/reach-ownership-split.md)
  — ships this doc's model: the description/ownership split, the holder-rule
  plumbing at every carrier position, and the deletion of the Done-boundary
  relocation channel.
