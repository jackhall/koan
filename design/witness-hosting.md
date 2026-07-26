# Witness sets: reach descriptions, carriers, and pins

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

## Description and pins: data versus liveness

Reach evidence is built from two ingredients, and nothing in the system
conflates them:

- The **reach description** answers *questions* — "which regions does this
  value borrow into?" (`pins_region`, membership queries). Its members are
  `Weak`; holding a description keeps **nothing** alive. It is pure data,
  arena-hosted beside the value it describes.
- **Owned pins** answer *liveness* — strong frame-owner `Rc<FrameStorage>`s,
  one per region the value's reach names. Holding a pin is what keeps a region —
  its arena, its side table, its scope's bindings — alive. Dropping it is what
  releases it.

The compile-safety line between them: a `Weak` member becomes an owned pin
**only by presenting a live pin**. The upgrade doors — `open_at` and the lift
(§ The carrier states) — take coverage by signature, so a holder that has only
a description cannot manufacture the pins that would keep the described regions
alive. Reading a description's `Weak` members is always a pinned read; a member
that fails to upgrade under its pin is a coverage bug — `debug_assert`ed in
debug, treated as non-pinning in release — never a source of an under-pinned
owned set.

## The carrier states

There is **one** value family — objects via
[`CarriedFamily`](../src/machine/model/values/carried.rs), functions via
[`KFunctionFamily`](../src/machine/core/kfunction.rs) (the witnessed
library is generic over `Reattachable` families, so a function is a family, not
a carrier variant). A carrier moves through **three states**, distinguished by
pointer strength and borrow posture and connected by *transform verbs* — never
by wrapping one in another:

| State | Reach members | Posture | Where it lives |
|---|---|---|---|
| [`Delivered`](../workgraph/src/witnessed/delivered.rs) | **owned** — `Rc`s in an inline set | in transit, borrows nothing | retention holds, node slots, dep terminals, and the pull / adopt / relocate verbs |
| [`Sealed`](../workgraph/src/witnessed.rs) | **weak** — a reference to the arena-hosted description | at rest, borrows nothing | binding-table entries, parked node slots |
| [`Opened<'b>`](../workgraph/src/witnessed.rs) | weak, read under a pin | in use at a step lifetime `'b` | within a step (`Resolved<'step>`, ATTR / schema reads) |

- `Delivered` owns its members outright, so it can **walk between frames**: the
  producer frame may die in transit, and an arena-hosted description reference
  would dangle, so a walking value carries strong pins for its whole reach.
- `Sealed` references the description in the value's home region's side table.
  That is sound at rest because the description dies with the region that hosts
  it; a `Sealed` carrier keeps nothing alive on its own (holder rule 1).
- `Opened<'b>` is a `Sealed` read **under a live pin**, borrowing at the pin's
  lifetime `'b`. It is the only state that can answer membership queries,
  because the pin it borrows is what makes reading the `Weak` members sound.

**The embedder has no pin vocabulary.** `PinBundle` is crate-private to the
library. Every owned pin lives in a library-owned holder — a retention hold, a
region's union bundle, a step's coverage — and the only shape one crosses the
boundary in is `StepCoverage`, an opaque holder the embedder may hold, thread and
drop but cannot compute with: union, subsumption, narrowing and member removal
have no public surface. Koan therefore cannot assemble, widen or narrow a claim.
The pinning invariant below is not a rule Koan is asked to honor but one it has no
vocabulary to break. What Koan supplies instead is *policy*: the retention
predicate at the escape seam (§ Escape) and the value-model queries a cost
decision runs on. Each of the transform verbs is exposed as a **container verb**
on the holder that owns the pins — a node slot, a region, a step — so the
container supplies the home owner the verb needs and Koan never has to recover a
producer region from a member set.

`Delivered` stays **nameable** by the embedder, and is the one state that is. An
in-transit envelope has to appear in the embedder's own type positions — a parked
node slot, a dep terminal, a finish callback's result — and a step's dep slice must
*own* its envelopes across a step that mutably borrows the scheduler, so an
`Opened<'b>` borrowed from the retention hold would not type. Nameable is not
transparent: the envelope's pins have no public accessor, no constructor takes a
bare bundle, and every operation on them is a verb on the envelope itself.

The transform verbs:

- **`Sealed::open_at<'b>(&'b self, pin: &'b F) -> Opened<'b>`** — a borrow-tied
  read. `'b` rides the pin borrow (the live frame), so no rank-2 closure is
  needed. `Delivered::open_at` is the one-line convenience that supplies its own
  owned coverage.
- **`Opened::reseal() -> Sealed`** — the step-end return to rest. Sound because
  `Opened` is `Copy` and constructible only by opening, so the value↔reach
  pairing it reseals is exactly the one it was opened from.
- **The lift, `Sealed → Delivered`** — read the description under still-live
  ambient coverage and upgrade the claimed members `Weak → Rc` into an owned
  inline set. This is the one place weak becomes strong; the ambient pin gates
  it, so a `Sealed` with no live coverage cannot be lifted. The set is owned
  inline (not a borrow of the arena description) precisely because the source
  frame may die once the value is walking.
- **The adopt, `Delivered → Sealed`** — mint a frozen description into the
  destination arena, retain the owned members into the destination's liveness
  (§ The pin bundle), and resolve the resting carrier's members to weak.
  `Delivered::open_adopted` is the same verb landing in `Opened<'d>` instead of
  at rest: `'d` is the *destination region's* own lifetime rather than a pin
  borrow, which is sound because the retained bundle covers that region's whole
  life. That is what lets an adopted value ride a step-lifetime type position no
  pin borrow reaches — dispatch's picked overload becomes an
  `Opened<'step, KFunctionFamily>` this way, carried by `Resolved<'step>` across
  argument evaluation and `reseal`ed into the `ReturnContract` that escapes into
  the call chain.
- **`Delivered::project`** re-families an envelope *in place* — no mint, no copy,
  no relocation. The envelope keeps its residence, coverage and witness, which
  stay correct because the projection selects a part **of** the value the
  envelope already covers (the callable a bound value wraps), so it can reach
  nothing the whole did not. It is how a family-specific carrier is reached
  without splitting the value from its pins; a projection taken through a bare
  read would arrive somewhere else with no proven reach at all.

Home rides as an **ordinary member**, never a distinguished field. A carrier's
reach is one flat set, and "the value borrows into the region it lives in" is
just that region appearing as a member. The single place home is treated
specially is the owned-upgrade boundary, one region-identity rule: **never pin a
region into itself** (§ Composition). A `Weak` member naming a carrier's own
home closes no strong cycle, so home sits in the description like any other
member; only an *owned* pin naming the holder's own region would make the region
transitively own itself, and that is the pin the self rule drops.

## The reach description

- Descriptions live in an **append-stable side table owned by the region's
  `FrameStorage`** — ordinary heap data, dropped when the region drops. They are
  **not** arena-page data, so arena pages carry no `Drop`-bearing reach state
  ([value-substrates.md](value-substrates.md)'s untyped `Drop`-free end state
  needs the pages clean). The table is append-only in address: a description's
  `&` stays valid for the region's whole life, which is what lets a `Sealed`
  carrier hold a thin reference to it.
- A description is **per-object and precise**: the exact reach of one stored
  value, home included. There is no whole-region merged description — two values
  in one region with different reaches reference two different entries.
- A description is **frozen at mint**. No site mutates one; composition mints a
  new entry. This is load-bearing: `Sealed` carriers share references to one
  description, so growing a shared one would silently widen every sharing
  carrier's claimed reach, and shrinking one would falsify a claim some carrier
  still relies on.
- The description is **not a storage family**: it is not allocated through the
  value store path and carries no `Stored`/`Reattachable` bounds. Only values
  live in arenas; reach metadata lives beside them, in the table.
- **Empty reach is `None`, not a hosted empty entry.** A region-pure value's
  description is `None` and it owns no pins — a region-pure bind allocates
  nothing and refcounts nothing. `None` **is** the empty set; every reader
  treats it as "reaches nothing," never "not yet computed."

## The pin bundle

Owned pins are collected in a **pin bundle** — working shape
`Vec<Rc<FrameStorage>>` (the concrete container is an identifier-level choice,
not part of the shape commitment). One frame owner per distinct region the
covered reach names. Where owned pins live:

- **A `Delivered` carrier's inline set.** A walking value — a node slot's
  terminal lifted for a pull, a dep crossing steps — carries its own pins, so
  every fan-out consumer (staged-sub splices, catch continuations, spliced
  expression clones) holds its own liveness. Duplicating a `Delivered`
  duplicates its pins. The set has no public accessor: it exists only for the
  library verbs the envelope exposes.
- **The region union bundle.** A region owns **one** deduped `PinBundle<F>`
  ([`Region::retain_reach`](../workgraph/src/witnessed/region.rs)); each bind and
  each copy-free adoption unions its pins into it. Binding entries themselves own
  nothing. This is the liveness of every value resident in the region: one owning
  pin per distinct foreign region across all of it, dropped whole at region
  death. It is region-owned rather than scope-owned because bindings are
  bind-once and a scope's entries never die before its region does, so the two
  schedules coincide — and a region is a library type, which is what keeps the
  union out of the embedder's hands.
- **The retention hold.** A finalized node slot's hold pairs the producer frame
  owner with owned pins for every *other* region the terminal reaches —
  `{ owner, reach, pulls }` — released together at pull-count zero
  (§ Retention model). `owner` is the home pin; `reach` never re-pins that same
  region (the self rule). This is the parked terminal's ownership; the slot's
  resting `Sealed` carrier is inert data without it.
- **Transient pins.** Short function-scope holds that open carriers — the run
  loop's per-step combined pin, the spliced-return check — hold explicit `Rc`s
  for exactly their scope. They never treat a description as a pin: a
  description pins nothing.

## The holder rule (the pinning invariant)

The one contract every open and every lift discharges against, numbered for
implementors:

1. A reach description keeps nothing alive. Ever. Any reasoning of the form
   "the description names region R, therefore R is alive" is wrong.
2. Regions are kept alive by owned pins (and by a region's own internal
   containment). Holding a region's owner `Rc` keeps that region — its arena,
   its side table, its scope's bindings — alive; that region's own union bundle
   keeps *its* reaches alive, recursively. Transitive coverage flows through
   **region union bundles**, not through descriptions.
3. Every holder of a carrier that can re-anchor its value either **owns pins
   covering the carrier's full reach** for its whole hold (a `Delivered`'s
   inline set; a region's union bundle covering its resident entries), or is
   **enveloped**: it lives strictly inside the lifetime of another holder's pins
   that cover the same reach (an `Opened<'b>` under a step pin, typed at the
   step's `'step` lifetime by
   [`StepCarried`](../src/machine/execute/step_carried.rs); an entry read under
   its region's own bundle). "Enveloped" is a lifetime claim the borrow checker
   can see, not a convention.
4. The open and lift doors — and every constructor that stores ownership (the
   retention hold, a region's adopt) — take the pin (or the enveloping borrow) by
   signature. A `Sealed` carrier plus no coverage is inert data, and stays
   inert: no operation turns its description into pins without a live pin
   (§ Description and pins).
5. Release is ordinary `Drop` of a pin bundle. There is no release verb, no
   un-mint, no audit. When a region dies, its union bundle drops and its pins
   with it. Bindings are bind-once, so an entry never drops by overwrite; a
   region's pins drop as a whole at region death.

Every one of these is a library-internal obligation: the holders rule 3 names are
all library types (§ The carrier states), so an embedder has no way to construct a
holder that could violate it.

## Composition: minting a description and retaining its pins

Every union — a merge of two carriers, a bind fold, a finalize reseal — mints a
**new frozen description into the destination region's side table** and retains
the composed owned pins into the destination's liveness. The mint verbs take the
destination's allocation capability; every composing site has one in hand (a
step finish: the consumer's region, held by the scheduler for the step —
guarantee 4 of [scheduler-library.md](scheduler-library.md#the-guarantees); a
scope bind or adoption: the scope's region owner).

Sources arrive as `Delivered` carriers whose members are already owned `Rc`s, so
the mint composes from **owned** members — the one weak→strong upgrade is
localized to the lift, gated by ambient coverage, so a member whose pin was
missed cannot silently slip into the composed set. A `Sealed` source is lifted
first. The mint reads its sources **precisely** — a value's witness never
coarsens to "everything its host region reaches." It then applies, against the
destination:

1. **The self rule** — the destination's own region is never an *owned* pin in a
   bundle stored resident in it (the self-ownership cycle, § The carrier
   states). A lift into a *foreign* destination upgrades the source's home like
   any member; a retain into the value's own home skips it. Description
   membership stays exact: home present as a `Weak` member means "borrows reach
   home," independent of whether that region is pinned here.
2. **Subsumption** — a member whose region another member's `pins_region` owner
   chain already keeps alive is dropped, so the bundle stays an antichain of the
   deepest owners. The subsumption hook is the embedder's
   [`PinsRegion`](../workgraph/src/witnessed/reach.rs) impl;
   `PinBundle::union` applies it on every insert.

Those two are the **only** shrinks. A mint applies no destination-relative
omission: a region the destination's frame or lexical chain already pins is
still a member and still an owned pin. Pinning it costs a refcount and nothing
else — the chain that made it ancestral holds it for at least as long as the
destination's own region lives, so an ancestor pin lengthens no lifetime. In
exchange a description is **exact**: it names the value's whole reach, not its
reach relative to some destination's ambient coverage. That exactness is what
makes a `Sealed` carrier self-describing — the lift can upgrade it to owned pins
with no policy threaded in from the embedder, and a reach query answers about the
value rather than about the pairing of a value with a reader. Subsumption still
collapses the redundancy whenever the covering ancestor is *itself* a member,
which is the common case.

A **pure pass-through** — a value returned up the call stack unmodified — runs no
mint: its carrier rides by reference (`Sealed`) or by ownership (`Delivered`)
unchanged, so a closure handed up N frames costs zero mints and zero refcount
traffic beyond the value's travel. A mint runs only where a value's reach is
genuinely restated against a new region — a bind, an adoption, a merge.

Acyclicity of the region ownership graph rests on two rules: the self rule (no
owned self-edge) and the per-call frame rule that a frame's `outer` chain
strong-owns only a **strictly older** ancestor frame — a DAG, never a back-edge —
so a dispatched frame chaining its (possibly per-call) captured parent forms no
cycle ([per-call-region/](per-call-region/README.md)).

## Threading: how pins reach each holder

Ownership flows from each adopt to the holder that needs it, and every hand-off
is a clone from a holder that already owns the pins, so coverage is a local
data-flow fact at every site, not an ambient whole-program invariant. The
retention consequence: every pin lives in a holder that already exists with the
same death schedule, so threading extends no region's lifetime — frames still
free at last-holder drop, never at region death.

- **Bind.** The adopt at the bind seam mints the destination description and
  unions the value's owned pins into the destination region's union bundle; the
  binding entry stores the resting `Sealed` carrier and owns nothing (§ Scope and
  bindings). A resident read that lifts the value onward opens it under the
  region's coverage and lifts internally to a `Delivered` for the verb's span.
- **Merge / relocation.** The composition verbs return a `Delivered` whose
  inline set is the composed pins. A Done-arm product carries its `Delivered`
  across the step with its carrier
  ([`StepCarried`](../src/machine/execute/step_carried.rs)), so the seal at the
  Done boundary supplies pins it holds. A Done product with empty reach — a
  literal, a type carrier, a region-pure value, the majority of builtin Done
  sites — carries the empty set, which pins nothing and allocates nothing.
- **Finalize → park → pull.** The finalize boundary reseals the terminal to
  `Sealed` at rest in the slot and hands its owned pins to the scheduler, which
  houses them in the slot's retention hold — `{ owner, reach, pulls }`. A
  consumer pull lifts a fresh `Delivered` for travel by cloning `(owner, reach)`
  out of the hold. The hold releases both at pull-count zero — the same instant
  the bare `owner` release already implies, since the held owner pins the reach
  members transitively (owner → region → its union bundle) for exactly that
  interval. Housing the pins in the hold makes the parked terminal's coverage
  owned rather than transitive; it does not lengthen it.
- **Fan-out.** Duplicating a `Delivered` clones its inline pins (§ The pin
  bundle), so each fan-out consumer owns its hold; a consumer that re-homes the
  value adopts a fresh description against its own destination.

## Escape: the single seam

A value escapes its producer frame in exactly one place: the **bind seam**, where
a consumer binds the delivered value into a scope. There is no second escape
channel.

- A **declared return** (an FN's `-> :T`, a MATCH/TRY arm's contract) is checked
  and re-stamped **in place**, in the producer's own region, at the Done
  boundary. The check moves no bytes and re-homes nothing. The sealed return
  obligation is pure `Copy` data — the declared type is a run-region registry
  handle ([typing/type-registry.md](typing/type-registry.md)) and the error
  label is precomputed at seal — so the obligation references no region, holds no
  pin, and carries no relocation destination. Under TCO the obligation rides the
  tail chain keep-first and the check fires once, at the chain's end, exactly as
  [tail-call-optimization.md](tail-call-optimization.md) schedules it.
- An **undeclared return** ends the same way: the value stays in its producer
  frame; the scheduler's retention hold (§ Retention) keeps that frame alive
  until every consumer pulls.
- At the bind seam the consumer prices **copy against pin**
  ([`copy_delivered_substrate`](../src/machine/core/scope/reach.rs), the cost
  model of
  [value-substrates.md § Cost-driven copy](value-substrates.md#cost-driven-copy-the-optimization)):
  *copy* rebuilds the value in the destination region and lets the producer frame
  free at retention discharge; *pin* leaves the value in the producer's region and
  unions its pins into the destination region's union bundle, making that region
  the value's residence for the destination's life. Both are always legal; the
  choice is pure cost.

The **retention predicate** is how that choice reaches the pin arithmetic. A
relocation verb does not accept a claim about what the product still reaches — it
*derives* one, by calling an embedder predicate on the product **after** the fold
has built it:

```
still_borrows(product: T::At<'b>, source: &Region) -> bool
```

A `false` verdict drops the source region from the composed bundle, so the
producer frees at retention discharge; a `true` verdict keeps it, so the producer
transfers by hold. Deriving rather than accepting is the point: the claim is a
checked property of the bytes that exist, not a promise made before they were
written, and the one place a mistake would dangle is a predicate over a live value
instead of a bundle assembled by hand. It is also where the embedder makes its own
memory-versus-CPU tradeoff — a predicate that answers conservatively costs
retention, never soundness, so a workload may tune it freely in either direction
without the library having to trust an ownership decision it cannot see.

Because pins are region-owned, a pinned residence ends when the binding's region
dies. The canonical example, spelled out:

```
FN count : n = MATCH (n) (0 -> 0) (_ -> count : n - 1)
```

Each tail hop retires its frame per retention. Bindings are bind-once and a tail
call is not known to re-enter the same function with a congruent slot set, so
each hop's `it` bind lands in a fresh scope: a loop-carried bind that priced to
**pin** would chain — iteration N+1's region bundle pins frame N, whose own region
bundle pins frame N−1, transitively. Every pin in that chain is droppable (each
dies with its scope's region), but a pinned loop holds O(N) retired regions until
[region evacuation](../roadmap/untyped_arena/region-evacuation.md) collapses the
chain at frame death. The bind seam therefore keeps its copy-bias for
loop-carried binds: the copy frees the producer at retention discharge,
preserving the O(1) region turnover TCO depends on.

Home = residence, by construction: a value is never moved out of its producer
region by any channel, so a `Delivered`'s home member, the producer's retention
hold, and the value's residence region are one and the same region. A carrier
whose home member does not name its value's residence cannot be built.

## Retention model

The lifetime of a **host frame** is the scheduler's frame-retention: the
scheduler holds a producer frame's owner `Rc` — paired with the terminal's owned
reach pins, `{ owner, reach, pulls }` — until every destination of its terminals
has pulled; release of both halves is a function of deliveries only, never of any
value's reach. A walking terminal carries this hold inside its `Delivered`
carrier, so the pins travel with the value to each consumer.

- A **pass-through** value stays hosted in its birth frame and rides up by
  reference; the birth frame is retained across the whole return chain and freed
  once the value is copied out or the last region pinning it drops.
- The **run region** is the residual: pins in the run region's bundle keep their
  members for the program. The lever that keeps this small is precision at the
  mint — a region-pure value's set is empty and pins nothing.
- **Region death** drops the region's side table and its union bundle — and
  therefore every pin that bundle owns. Refcount decrements for a region's
  outbound pins batch at that teardown.

**TCO** consumes retention directly: a tail call reinstalls the slot's work, the
retiring incarnation's region is held by retention until the reinstalled
incarnation adopts its sealed arguments, and the free is ordered after the
adoption. The full design is
[tail-call-optimization.md](tail-call-optimization.md).

## Scope and bindings above the substrate

The Koan layers compose the substrate; neither the scope nor its binding entries
hold any witness state — the pins live one level down, in the library's region.

- **Binding entries are `Copy` and `Drop`-free.** Both binding tables store one
  `Sealed`-shaped carrier beside a `BindingIndex` — `data: name → (BindingIndex,
  Sealed<CarriedFamily>)` and `functions: key → Vec<(BindingIndex,
  Sealed<KFunctionFamily>)>` — so a value is never separated from the reach that
  proves it. An entry owns no pins; its liveness is the region's union bundle.
- **The scope holds no pin state at all.** Binding a value mints its description
  into the scope's region and unions its pins into that **region's** own deduped
  bundle (§ The pin bundle), applying the self rule before insertion.
  `PinBundle::union` dedupes by region identity with outer-chain subsumption, so
  the region carries one pin per distinct foreign region, not one bundle per
  entry. The mint and the store are **one fused door** (`Scope::bind_delivered` /
  `bind_checked`, `bind_module`, `register_type_delivered` and siblings), so a
  scope entry cannot state a reach the value's borrows don't back, and the union
  is written by the library rather than by the door's caller.
- **Reads stay refcount-free.** A binding read opens the entry's `Sealed` under
  the region's own coverage (`open_at`) and hands out an `Opened<'b>` enveloped by
  the region's union bundle (holder rule 3); pins are adopted only when the value
  genuinely escapes to a new holder — a new region.
- **Module reach** is the union over the child scope's entries, minted once at
  scope close; the parent region's union bundle owns the resulting pins.

## Residence enforcement

A composite value's **residence** — every region its borrows reach is covered by
the destination — is discharged **at construction, by the fold brand**, not by a
runtime walk. A relocation or bind builds the value inside a `for<'b>` fold
closure ([`FoldingBrand::alloc_object_folded`](../src/machine/core/arena.rs)),
where the only inhabitants of `KObject<'b>` are the fold's declared operand
views, the brand's own allocations, and owned data — all named by the witness the
enclosing combinator composes. An ambient-lifetime capture is a compile error at
the closure signature, so the store is sound with no per-value audit. Copy and
pin both take this door: a copy's fold structurally rebuilds into the
destination; a pin's fold pointer-copies the source under the composed `Kept`
witness that names its producer host. This is the residence analogue of the
description/pins compile-safety line (§ Description and pins) — a move-in that
cannot name its reach does not typecheck.

A small set of dest-only runtime residence checks remain, each covering a
property no carrier lifetime captures, and each a **backstop** rather than the
enforcement tier:

- **Splice-free gate.** A `KObject::KExpression` moved in as data is vetted by
  the dest-only [`resident_in`](../src/machine/model/values/kobject.rs) walk,
  which rejects a spliced expression carrying a producer reach the empty seal
  cannot name. Splice-freeness is a runtime data property no carrier lifetime
  distinguishes.
  [Drop-free region death](../roadmap/untyped_arena/drop-free-region-death.md)
  removes this last dest-only walk once expression parts are `Drop`-free.
- **Primitive reattach guards.** A `KFunction`, `Scope`, or `Module` borrows a
  single region (its captured / parent / child scope); the `ptr::eq` guard on its
  arena move-in checks that region is the destination — the reattach witness for
  the `'_ → 'a` erasure. These are single-region checks, not the composite
  reaching tier.

## Library boundary

Per the [scheduler-library.md](scheduler-library.md) division:

- **Library-owned:** the description and pin-bundle types; the three carrier
  states and their transform verbs (`open_at`, `reseal`, the lift, the adopt) on
  the [`Carrier`](../workgraph/src/witnessed/carrier.rs) /
  [`Delivered`](../workgraph/src/witnessed/delivered.rs) types; the
  `KFunctionFamily`; the mint mechanism (freeze-at-mint, the self rule,
  subsumption fold) which requires a destination allocation capability by
  signature; the single-seam escape verb
  [`restamp_in_place`](../workgraph/src/witnessed/delivered.rs) (re-tags a
  delivered value's top node in its own producer region, composing to a witness
  identical to the input's); the scheduler's frame-retention (release at
  pull-count zero); and **every owned pin** — the region union bundles, the
  retention holds, the step coverages. `PinBundle` is crate-private and an
  envelope's pins have no accessor, so the whole ownership tier is unreachable
  from outside even though `Delivered` itself is nameable (§ The carrier states).
- **Workload-supplied:** the frame-owner type `F` with its `PinsRegion`
  subsumption hook, and the retention predicate at relocation sites (§ Escape) —
  policy inputs, never pins.

## Open work

- [Residence-audit retirement](../roadmap/untyped_arena/residence-audit-retirement.md)
  — routes the reaching-tier move-ins through the fold-brand construction door
  (§ Residence enforcement) and deletes the runtime reaching audit; the backstops
  there are what remains.
- [Binding tables as witnessed carriers](../roadmap/untyped_arena/binding-tables-witnessed-carriers.md)
  — implements the three-carrier model this doc describes: both binding tables
  store the `Sealed` carrier, home becomes an ordinary `Weak` member, and the
  scope's single union bundle replaces per-entry pins.
