# Reach: descriptions, carriers, and pins

This doc owns the **representation and ownership model of reach evidence**: how a
value's reach is described, what keeps the regions it reaches alive, and which
holder owns that keeping-alive. [witnessed-memory.md](witnessed-memory.md) owns
the carrier construction and access mechanics this representation slots under;
[sectioned-reach.md](sectioned-reach.md) refines it to sub-value granularity;
[scheduler-library.md](../../design/scheduler-library.md) owns the
library/embedder boundary the types sit on. Type names here are working names —
shapes are the commitment, identifiers are not.

A value's **reach** is the set of regions its borrows can land in. Held as
evidence, it is the value's liveness witness.

## Description and pins: data versus liveness

Reach evidence is built from two ingredients, and nothing in the system conflates
them:

- The **reach description** answers *questions* — "which regions does this value
  borrow into?" (`pins_region`, membership queries). Its members are `Weak`;
  holding a description keeps **nothing** alive. It is pure data, hosted beside
  the value it describes.
- **Owned pins** answer *liveness* — strong region-owner handles, one per region
  the value's reach names. Holding a pin is what keeps a region — its arena, its
  side table, whatever the embedder stores in it — alive. Dropping it is what
  releases it.

The compile-safety line between them: a `Weak` member becomes an owned pin **only
by presenting a live pin**. The upgrade doors — `open_at` and the lift
(§ The carrier states) — take coverage by signature, so a holder that has only a
description cannot manufacture the pins that would keep the described regions
alive. Reading a description's `Weak` members is always a pinned read; a member
that fails to upgrade under its pin is a coverage bug — `debug_assert`ed in debug,
treated as non-pinning in release — never a source of an under-pinned owned set.

## The carrier states

The substrate is generic over `Reattachable` families, so a workload's distinct
value kinds are distinct *families* rather than variants of one carrier. A carrier
moves through **three states**, distinguished by pointer strength and borrow
posture and connected by *transform verbs* — never by wrapping one in another:

| State | Reach members | Posture | Where it lives |
|---|---|---|---|
| [`Delivered`](../src/witnessed/delivered.rs) | **owned** — region owners in an inline set | in transit, borrows nothing | retention holds, cell slots, dep terminals, and the pull / adopt / relocate verbs |
| [`Sealed`](../src/witnessed.rs) | **weak** — a reference to the region-hosted description | at rest, borrows nothing | embedder binding tables, parked cell slots |
| [`Opened<'b>`](../src/witnessed.rs) | weak, read under a pin | in use at a step lifetime `'b` | within a step |

- `Delivered` owns its members outright, so it can **walk between regions**: the
  producer region may die in transit, and a region-hosted description reference
  would dangle, so a walking value carries strong pins for its whole reach.
- `Sealed` references the description in the value's home region's side table.
  That is sound at rest because the description dies with the region that hosts
  it; a `Sealed` carrier keeps nothing alive on its own (holder rule 1).
- `Opened<'b>` is a `Sealed` read **under a live pin**, borrowing at the pin's
  lifetime `'b`. It is the only state that can answer membership queries, because
  the pin it borrows is what makes reading the `Weak` members sound.

**The embedder has no pin vocabulary.** `PinBundle` is crate-private to the
library. Every owned pin lives in a library-owned holder — a retention hold, a
region's union bundle, a step's coverage — and the only shape one crosses the
boundary in is `StepCoverage`, an opaque holder the embedder may hold, thread and
drop but cannot compute with: union, subsumption, narrowing and member removal
have no public surface. An embedder therefore cannot assemble, widen or narrow a
claim. The pinning invariant below is not a rule the embedder is asked to honor
but one it has no vocabulary to break. What the embedder supplies instead is
*policy*: the retention predicate at relocation seams and the value-model queries
a cost decision runs on. Each of the transform verbs is exposed as a **container
verb** on the holder that owns the pins — a cell slot, a region, a step — so the
container supplies the home owner the verb needs and the embedder never has to
recover a producer region from a member set.

`Delivered` stays **nameable** by the embedder, and is the one state that is. An
in-transit envelope has to appear in the embedder's own type positions — a parked
cell slot, a dep terminal, a finish callback's result — and a step's dep slice
must *own* its envelopes across a step that mutably borrows the scheduler, so an
`Opened<'b>` borrowed from the retention hold would not type. Nameable is not
transparent: the envelope's pins have no public accessor, no constructor takes a
bare bundle, and every operation on them is a verb on the envelope itself.

The transform verbs:

- **`Sealed::open_at<'b>(&'b self, pin: &'b F) -> Opened<'b>`** — a borrow-tied
  read. `'b` rides the pin borrow (the live region owner), so no rank-2 closure is
  needed. `Delivered::open_at` is the one-line convenience that supplies its own
  owned coverage.
- **`Opened::reseal() -> Sealed`** — the step-end return to rest. Sound because
  `Opened` is `Copy` and constructible only by opening, so the value↔reach pairing
  it reseals is exactly the one it was opened from.
- **`Opened::lift_out() -> Delivered`** — the relocation seam, `reseal` composed
  with the lift under the description's **own host**. It is the verb a value
  parted from a container takes to travel
  ([sectioned-reach.md](sectioned-reach.md)): the projection is `'b`-confined by
  its type and states exactly its own reach, and this is the one place that reach
  becomes owned. Deriving the residence from the description rather than from an
  argument is what keeps a caller from pairing a value with a home it did not
  derive.
- **The lift, `Sealed → Delivered`** — read the description under still-live
  ambient coverage and upgrade the claimed members `Weak → strong` into an owned
  inline set. This is the one place weak becomes strong; the ambient pin gates it,
  so a `Sealed` with no live coverage cannot be lifted. The set is owned inline
  (not a borrow of the hosted description) precisely because the source region may
  die once the value is walking.
- **The adopt, `Delivered → Sealed`** — mint a frozen description into the
  destination region, retain the owned members into the destination's liveness
  (§ The pin bundle), and resolve the resting carrier's members to weak.
  `Delivered::open_adopted` is the same verb landing in `Opened<'d>` instead of at
  rest: `'d` is the *destination region's* own lifetime rather than a pin borrow,
  which is sound because the retained bundle covers that region's whole life. That
  is what lets an adopted value ride a step-lifetime type position no pin borrow
  reaches.
- **`Delivered::project`** re-families an envelope *in place* — no mint, no copy,
  no relocation. The envelope keeps its residence, coverage and witness, which stay
  correct because the projection selects a part **of** the value the envelope
  already covers, so it can reach nothing the whole did not. It is how a
  family-specific carrier is reached without splitting the value from its pins; a
  projection taken through a bare read would arrive somewhere else with no proven
  reach at all.

A description records **two** facts about one value, and they are not the same
fact: its `host` is the region the value **lives in** (its residence), its
`members` are the regions the value's borrows **reach**. Home appears in `members`
only when the value genuinely borrows into its own region — living in a region is
not borrowing into it, so a region-pure scalar has a real host and an empty member
set. "Does this value borrow into its own home?" is therefore the ordinary
membership query `members ∋ host`, answered on an `Opened` like any other
membership question, and residence is read off that same record rather than off a
side channel on whatever is holding the value.

The single place home is treated specially is the owned-upgrade boundary, one
region-identity rule: **never pin a region into itself** (§ Composition). It
applies to the owned bundle alone. A `Weak` naming a carrier's own home — as the
`host`, or as a member when the borrows do reach it — closes no strong cycle; only
an *owned* pin naming the holder's own region would make the region transitively
own itself, and that is the pin the self rule drops.

## The reach description

- Descriptions live in an **append-stable side table owned by the region's
  storage** — ordinary heap data, dropped when the region drops. They are **not**
  arena-page data, so arena pages carry no `Drop`-bearing reach state. The table
  is append-only in address: a description's `&` stays valid for the region's whole
  life, which is what lets a `Sealed` carrier hold a thin reference to it.
- A description is **precise**, and the table **interns** it
  ([sectioned-reach.md § Interned side table](sectioned-reach.md)): an entry is
  the exact reach of the values referencing it, keyed on that member set, so one
  entry exists per distinct reach per region. There is no whole-region merged
  description — two values in one region with different reaches reference two
  different entries — and, conversely, two values with the same reach reference
  one. Within a region a description's *address* is therefore its member set,
  which is what makes an equality test over reach a pointer compare.
- A description is **frozen at mint**. No site mutates one; composition get-or-mints
  an entry and never edits the one it finds. This is load-bearing: `Sealed` carriers
  share references to one description, so growing a shared one would silently widen
  every sharing carrier's claimed reach, and shrinking one would falsify a claim
  some carrier still relies on.
- The description is **not a storage family**: it is not allocated through the
  value store path and carries no `Stored`/`Reattachable` bounds. Only values live
  in arenas; reach metadata lives beside them, in the table.
- **Every value has a description**, because that is where its residence is
  recorded. A region-pure value references one whose members are empty — one `Weak`
  host and an empty member list, no heap — and owns no pins, so a region-pure bind
  refcounts nothing. Empty members **are** the empty reach set; every reader treats
  them as "reaches nothing," never "not yet computed," and asks "reaches anything?"
  of the members rather than of the description's existence.

## The pin bundle

Owned pins are collected in a **pin bundle** — one region owner per distinct
region the covered reach names. Where owned pins live:

- **A `Delivered` carrier's inline set.** A walking value — a cell slot's terminal
  lifted for a pull, a dep crossing steps — carries its own pins, so every fan-out
  consumer holds its own liveness. Duplicating a `Delivered` duplicates its pins.
  The set has no public accessor: it exists only for the library verbs the envelope
  exposes.
- **The region union bundle.** A region owns **one** deduped `PinBundle<F>`
  ([`Region::retain_reach`](../src/witnessed/region.rs)); each bind and each
  copy-free adoption unions its pins into it, filtered by the eternal rule
  (§ Composition) and folded **once per distinct reach** — a second retention
  naming a member set this region already pins is a no-op
  ([sectioned-reach.md § Interned side table](sectioned-reach.md)).
  Embedder binding entries themselves own nothing. This is the
  liveness of every value resident in the region: one owning pin per distinct
  foreign region across all of it, dropped whole at region death. It is
  region-owned rather than embedder-owned because a region-lifetime union is
  exactly as tight as a per-entry bundle whenever entries never outlive the region
  — and a region is a library type, which is what keeps the union out of the
  embedder's hands.
- **The retention hold.** A finalized cell slot's hold pairs the producer region
  owner with owned pins for every *other* region the terminal reaches —
  `{ owner, reach, pulls }` — released together at pull-count zero
  (§ Retention model). `owner` is the home pin; `reach` never re-pins that same
  region (the self rule). This is the parked terminal's ownership; the slot's
  resting `Sealed` carrier is inert data without it.
- **Transient pins.** Short function-scope holds that open carriers — a run loop's
  per-step combined pin, an embedder's in-flight check — hold explicit owners for
  exactly their scope. They never treat a description as a pin: a description pins
  nothing.

## The holder rule (the pinning invariant)

The one contract every open and every lift discharges against, numbered for
implementors:

1. A reach description keeps nothing alive. Ever. Any reasoning of the form "the
   description names region R, therefore R is alive" is wrong.
2. Regions are kept alive by owned pins (and by a region's own internal
   containment). Holding a region's owner keeps that region — its arena, its side
   table, its stored contents — alive; that region's own union bundle keeps *its*
   reaches alive, recursively. Transitive coverage flows through **region union
   bundles**, not through descriptions.
3. Every holder of a carrier that can re-anchor its value either **owns pins
   covering the carrier's full reach** for its whole hold (a `Delivered`'s inline
   set; a region's union bundle covering its resident entries), or is
   **enveloped**: it lives strictly inside the lifetime of another holder's pins
   that cover the same reach (an `Opened<'b>` under a step pin; an entry read under
   its region's own bundle). "Enveloped" is a lifetime claim the borrow checker can
   see, not a convention.
4. The open and lift doors — and every constructor that stores ownership (the
   retention hold, a region's adopt) — take the pin (or the enveloping borrow) by
   signature. A `Sealed` carrier plus no coverage is inert data, and stays inert:
   no operation turns its description into pins without a live pin
   (§ Description and pins).
5. Release is ordinary `Drop` of a pin bundle. There is no release verb, no
   un-mint, no audit. When a region dies, its union bundle drops and its pins with
   it.

Every one of these is a library-internal obligation: the holders rule 3 names are
all library types (§ The carrier states), so an embedder has no way to construct a
holder that could violate it.

## Composition: minting a description and retaining its pins

Every union — a merge of two carriers, a bind fold, a finalize reseal — gets-or-mints
a **frozen description in the destination region's side table** and retains the
composed owned pins into the destination's liveness. Both halves dedupe: an already
described member set yields the existing entry, and a member set the destination
already pins folds nothing. The mint verbs take the
destination's allocation capability, so a composing site must hold one: inside a
step, the consumer's region held by the scheduler
([guarantee 4](../../design/scheduler-library.md#the-guarantees)); outside one, the
destination region's owner.

Sources arrive as `Delivered` carriers whose members are already owned, so the
mint composes from **owned** members — the one weak→strong upgrade is localized to
the lift, gated by ambient coverage, so a member whose pin was missed cannot
silently slip into the composed set. A `Sealed` source is lifted first. The mint
reads its sources **precisely** — a value's witness never coarsens to "everything
its host region reaches." It then applies, against the destination:

1. **The self rule** — the destination's own region is never an *owned* pin in a
   bundle stored resident in it (the self-ownership cycle, § The carrier states). A
   lift into a *foreign* destination upgrades the source's home like any member; a
   retain into the value's own home skips it. Description membership stays exact:
   home present as a `Weak` member means "borrows reach home," independent of
   whether that region is pinned here.
2. **Subsumption** — a member whose region another member's `pins_region` owner
   chain already keeps alive is dropped, so the bundle stays an antichain of the
   deepest owners. The subsumption hook is the embedder's
   [`PinsRegion`](../src/witnessed/reach.rs) impl; `PinBundle::union` applies it on
   every insert.

Those two are the **only** shrinks. A mint applies no destination-relative
omission: a region the destination's own chain already pins is still a member and
still an owned pin. Pinning it costs a refcount and nothing else — the chain that
made it ancestral holds it for at least as long as the destination's own region
lives, so an ancestor pin lengthens no lifetime. In exchange a description is
**exact**: it names the value's whole reach, not its reach relative to some
destination's ambient coverage. That exactness is what makes a `Sealed` carrier
self-describing — the lift can upgrade it to owned pins with no policy threaded in
from the embedder, and a reach query answers about the value rather than about the
pairing of a value with a reader. Subsumption still collapses the redundancy
whenever the covering ancestor is *itself* a member, which is the common case.

A **pure pass-through** — a value handed onward unmodified — runs no mint: its
carrier rides by reference (`Sealed`) or by ownership (`Delivered`) unchanged, so a
value handed up N frames costs zero mints and zero refcount traffic beyond its
travel. A mint runs only where a value's reach is genuinely restated against a new
region — a bind, an adoption, a merge.

### The eternal rule

The self rule bounds a *mint*. A second rule bounds a **region-lifetime
retention**: a member whose owner declares
[`PinsRegion::needs_no_pin`](../src/witnessed/reach.rs) — storage that already
outlives every region that could retain it — never enters a region's union bundle
(`PinBundle::without_eternal`, applied at `Region::retain_reach`). Naming an
eternal tier is the embedder's call; the library only asks the question.

The rule is applied at retention and nowhere else, because only a region-lifetime
pin can close a ring. A transient bundle — a binding entry's lift, a delivery
envelope, a step's coverage — is left alone; an extra refcount there closes
nothing. The *description* is untouched either way, so reach membership stays exact
and every residence query still sees the eternal region as a named member.

The self rule alone does not suffice, because an owner outside the destination's
own region can still hold a chain back to it. The concrete ring: a short-lived
region adopting an eternal-region-resident value takes an owning pin on the eternal
host, while the eternal region adopting that computation's result takes an owning
pin on the short-lived host. Neither edge alone leaks; the pair is a cycle that no
owner-chain walk sees, because the short-lived region's chain terminates and the
ring is expressed entirely through reach. The eternal rule cuts it at the eternal
edge.

Its correctness obligation is a **drop-order** one, and it is the reason
`needs_no_pin` sits under the `unsafe PinsRegion` contract: answering `true`
asserts that the owner's storage stays live and fixed-address for at least as long
as any region that could retain it.

Acyclicity of the region ownership graph therefore rests on three rules: the self
rule (no owned self-edge), the eternal rule (no owned edge into eternal storage),
and the embedder's own obligation that a region owner's chain strong-owns only a
**strictly older** ancestor — a DAG, never a back-edge.

## Threading: how pins reach each holder

Ownership flows from each adopt to the holder that needs it, and every hand-off is
a clone from a holder that already owns the pins, so coverage is a local data-flow
fact at every site, not an ambient whole-program invariant. The retention
consequence: every pin lives in a holder that already exists with the same death
schedule, so threading extends no region's lifetime — regions still free at
last-holder drop, never at some later death.

- **Bind.** The adopt at an embedder's bind seam mints the destination description
  and unions the value's owned pins into the destination region's union bundle; the
  binding entry stores the resting `Sealed` carrier and owns nothing. A resident
  read that lifts the value onward opens it under the region's coverage and lifts
  internally to a `Delivered` for the verb's span.
- **Merge / relocation.** The composition verbs return a `Delivered` whose inline
  set is the composed pins. A step's product carries its `Delivered` across the
  step with its carrier, so the seal at the step boundary supplies pins it holds. A
  product with empty reach carries the empty set, which pins nothing and allocates
  nothing.
- **Finalize → park → pull.** The finalize boundary reseals the terminal to
  `Sealed` at rest in the slot and hands its owned pins to the scheduler, which
  houses them in the slot's retention hold — `{ owner, reach, pulls }`. A consumer
  pull lifts a fresh `Delivered` for travel by cloning `(owner, reach)` out of the
  hold. The hold releases both at pull-count zero — the same instant the bare
  `owner` release already implies, since the held owner pins the reach members
  transitively (owner → region → its union bundle) for exactly that interval.
  Housing the pins in the hold makes the parked terminal's coverage owned rather
  than transitive; it does not lengthen it.
- **Fan-out.** Duplicating a `Delivered` clones its inline pins (§ The pin bundle),
  so each fan-out consumer owns its hold; a consumer that re-homes the value
  get-or-mints a description against its own destination.

## Retention model

The lifetime of a **producer region** is the scheduler's retention: the scheduler
holds the producer's region owner — paired with the terminal's owned reach pins,
`{ owner, reach, pulls }` — until every destination of its terminals has pulled;
release of both halves is a function of deliveries only, never of any value's
reach. A walking terminal carries this hold inside its `Delivered` carrier, so the
pins travel with the value to each consumer.

- A **pass-through** value stays hosted in its birth region and rides onward by
  reference; the birth region is retained across the whole chain and freed once the
  value is copied out or the last region pinning it drops.
- **Region death** drops the region's side table and its union bundle — and
  therefore every pin that bundle owns. Refcount decrements for a region's outbound
  pins batch at that teardown.
- The **residual** is whatever the longest-lived region's bundle holds. The lever
  that keeps this small is precision at the mint — a region-pure value's set is
  empty and pins nothing.

**Slot reinstallation** consumes retention directly: reinstalling a slot's work
retires the outgoing incarnation's region, which retention holds until the
reinstalled incarnation adopts its sealed arguments, ordering the free after the
adoption.

Home = residence, by construction: a value is never moved out of its producer
region by any channel, so a `Delivered`'s home member, the producer's retention
hold, and the value's residence region are one and the same region. A carrier whose
home member does not name its value's residence cannot be built.

## The library boundary

- **Library-owned:** the description and pin-bundle types; the three carrier states
  and their transform verbs (`open_at`, `reseal`, the lift, the adopt) on the
  [`Carrier`](../src/witnessed/carrier.rs) /
  [`Delivered`](../src/witnessed/delivered.rs) types; the mint mechanism
  (freeze-at-mint, the self rule, subsumption fold), which requires a destination
  allocation capability by signature; the in-place restamp verb
  [`restamp_in_place`](../src/witnessed/delivered.rs) (re-tags a delivered value's
  top node in its own producer region, composing to a witness identical to the
  input's); the scheduler's retention (release at pull-count zero); and **every
  owned pin** — the region union bundles, the retention holds, the step coverages.
  `PinBundle` is crate-private and an envelope's pins have no accessor, so the whole
  ownership tier is unreachable from outside even though `Delivered` itself is
  nameable (§ The carrier states).
- **Workload-supplied:** the region-owner type `F` with its `PinsRegion`
  subsumption hook and its `needs_no_pin` eternal-tier answer, and the retention
  predicate at relocation sites — policy inputs, never pins.

The **retention predicate** is how an embedder's copy-versus-pin choice reaches the
pin arithmetic. A relocation verb does not accept a claim about what the product
still reaches — it *derives* one, by calling the embedder predicate on the product
**after** the fold has built it:

```
still_borrows(product: T::At<'b>, source: &Region) -> bool
```

A `false` verdict drops the source region from the composed bundle, so the producer
frees at retention discharge; a `true` verdict keeps it, so the producer transfers
by hold. Deriving rather than accepting is the point: the claim is a checked
property of the bytes that exist, not a promise made before they were written, and
the one place a mistake would dangle is a predicate over a live value instead of a
bundle assembled by hand. It is also where the embedder makes its own
memory-versus-CPU tradeoff — a predicate that answers conservatively costs
retention, never soundness, so a workload may tune it freely in either direction
without the library having to trust an ownership decision it cannot see.
