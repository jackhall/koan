# The witnessed memory substrate

A cell is a long-lived object that nevertheless eventually dies, and between
birth and death it must hold values that *borrow* from memory it does not own.
The `witnessed` substrate is the generic, workload-independent machinery that
makes those borrow-carrying values safe to store, move, and read across a cell's
life — a bump allocator, a liveness witness, and a small carrier surface, naming
no embedder type.

The design goal is a single safe interface over per-cell memory: every access is
a borrow the compiler checks, and the substrate's own `unsafe` is confined to a
handful of audited lifetime retypes no caller can reach.

[reach.md](reach.md) owns the reach representation — what a carrier's witness
*names* and who owns the pins that keep it alive. This doc owns the carrier
construction and access mechanics that representation slots under.
[cellgraph.md](cellgraph.md) states the cell contract the substrate backs, and
[scheduler-library.md](../../design/scheduler-library.md) states the boundary an
embedder meets it at.

## The core: erase-store, witness, reattach

A value of type `T<'a>` cannot be stored in a structure that outlives `'a`. The
substrate stores its `'static`-erased form `T<'static>` instead — sound because a
lifetime is zero-sized, so `T<'a>` and `T<'static>` share layout — and re-anchors
a borrow on the way out. Three pieces carry the contract:

- `Reattachable` — an `unsafe` trait marking a family `{ type At<'r>; }` whose
  representation is identical across every choice of its one lifetime.
  `Erased<T>` stores `T::At<'static>`.
- `Witness` — an `unsafe` marker asserting its holder pins the value's backing at
  a fixed address while held (`StableDeref`-backed). A re-anchor is sound only
  while a witness is held.
- A single private `retype<A, B>` lifetime-cast, guarded by a size assert, is the
  only place a `T::At<'a> → T::At<'b>` retype is written. Every accessor routes
  it.

A family qualifies when its lifetime parameter names no live borrow — an
owned-data type whose `'a` is phantom, re-anchored invariantly by a zero-size
`PhantomData` marker, is layout-invariant and therefore reattachable exactly as a
genuinely borrowing family is.

### Two shapes of region-resident store

A region's value storage is its bump, and the bump is lifetime-free: `'a` enters only at the
allocating call. So a value whose every field is already at the caller's `'a` — its region
pointer included — is simply built there and placed, with **no brand, no erasure and no
`unsafe` at all**. [`BumpAllocator::in_place`](../src/witnessed/bump.rs) is that verb, for a
value that stays resident and keeps mutating through its interior-mutable fields (the shape
`Copy` cannot spell, since copying it would fork the shared mutable state). Residence is the
borrow checker's: the only region reachable through the caller's own handle is the one the
value lands in.

The one shape that needs more is a value embedding an operand borrowed from *outside* — a
reference into another region, which the caller holds at an enclosing lifetime the placement's
brand cannot see. [`RegionHandle::bump_born_with`](../src/witnessed/region.rs) is that door. It
takes a `for<'b>` construction closure and hands it a `FoldedPlacement` over this handle's own
region **plus** the operand, both re-anchored to that same `'b` — one `zip`ped `open`, because
branding the two independently is exactly what an invariant family rejects. The proof is the
quantifier: `'b` has no outlives relation to any enclosing lifetime, so the only
`&'b Region<W>` — and hence the only `X<'b>` built over one — the closure body can name is the
placement's. A captured ambient `&'a Region` does not coerce and does not compile, which makes
the built value's region pointer the destination's by construction. It is the same no-outlives
argument [`FoldedPlacement::allocator`](../src/witnessed.rs) rests on, with the operands'
reach left uncomposed.

What the signature still cannot prove is the operand's own liveness: its pointee may live in
another region, and the stored value keeps naming it for as long as the destination region
lives. The `pin` argument is where a caller discharges that, and it is borrowed for `'a` — the
destination region's own lifetime — so the `Witness` contract covers the stored reference's
whole life rather than merely the call. That keeps the door a lifetime *shortening*; the
residual co-location obligation (does this pin in fact cover this operand?) is the one every
`Witness` already carries, narrowed in duration rather than added to.

The door's one audited step is on the way out: the value is built and bumped at `'b`, and it
is the resulting **reference** that leaves the brand, erased through the generic
`ReferenceFamily<K>` and re-anchored to `'a`. That is a shortening onto live backing — the
pointee sits in a chunk the handle's `&'a Region` borrow pins for all of `'a` — and it routes
the substrate's single audited reattach rather than a retype of its own. The bumped *value* is
never erased or retyped at all.

Both doors restate the family's side of the bargain as a monomorphized
`const { assert!(!needs_drop::<…>()) }`: a bump runs no destructor, so what it hosts must have
none to run. A field that later grows a `Drop` is a build error at the store that admitted it.

## The bump allocator

`Region<P>` **is** its bump: a region's whole value storage is one `bumpalo::Bump`,
parameterized by a storage profile `P` that declares only the frame-owner type the
region's reach descriptions are typed at. There is no per-family cell and no
storage-policy trait for a workload to implement — a family declares its lifetime
shape and nothing else. Region death is chunk deallocation: no per-slot destructor
runs anywhere, which is what makes frame teardown O(1) rather than a walk.

That rests on every hosted family being `Drop`-free, and the substrate holds that
line statically rather than by audit: the placement verbs carry `T: Copy`, the two
non-`Copy` verbs carry a monomorphized `!needs_drop` assert, and a family declared
through the `reattachable!` default arm carries the same assert at its declaration.
A family that genuinely owns heap contents takes the `droppable` arm and rests in a
carrier that runs its glue, never in a region.

The bump is the home for the library's own container metadata (a sectioned
container's run partition and cell index block, see
[sectioned-reach.md](sectioned-reach.md)) *and* an embedder's value families, which
reach it through the same verbs. Two properties define the tier.

**It routes no erasure.** The allocator's own type carries no lifetime, so `'a`
enters only at the allocating call. A bumped value may therefore hold an `&'a`
back into the very region it lives in, with no `erase_to_static` and no residence
check — the thing a lifetime-*typed* cell could not do, because its slot type would
have to name a lifetime `Region` has no parameter for, forcing a
region-self-referential value's borrow through erasure and back through a
brand-confined reattach. Cycles among bumped entries are harmless: everything there
dies with the region, at once.

**`Copy` is the static proxy for "no destructor to skip".** A bump releases its
chunks whole and runs nothing, so a `Drop`-bearing entry would silently leak what
it owns. "`Drop`-free" has no expressible bound, so the write-once placement verbs
carry `T: Copy` instead — the honest approximation, and the bound that keeps a
sectioned container `Copy` and free at region teardown. The two stored shapes that
are glue-free without being `Copy` — a frozen table's header, and a value that keeps
mutating in place through interior mutability — take verbs of their own
(`frozen_table`, `in_place`) carrying a monomorphized `!needs_drop` assert, rather
than a relaxation of the `Copy` bound that would admit both and everything else too.

**The verbs are defined once, on the allocator handle.**
[`BumpAllocator<'b>`](../src/witnessed/bump.rs) is a `Copy`, brand-carrying
wrapper over a `Bump`, and it is where `value` / `slice` /
`slice_from_iter` / `text` live. Every surface that can reach a region's bytes — `RegionHandle::allocator`,
`FoldedPlacement::allocator`, an embedder's own brand veneer — hands back that
one type rather than restating a verb set of its own, so the `Copy` guard and its
rationale are written once. The wrapping constructor is `pub(crate)`, so a handle
exists only over a bump the library hands one out for, and `'b` is whatever brand
that hand-out chose: nothing built through one outlives the bump whose bytes it
holds. At this tier `'b` is the region's own brand, and where it is a rank-2 fold
brand that same lifetime is the confinement — which is why the allocator needs no
mint privacy of its own to serve as a fold's write surface.

The handle's other home is the scheduler's **step scratch** bump
([dag-scheduler.md § The drain protocol](dag-scheduler.md#the-drain-protocol)), whose
bytes are unpriced and reclaimed wholesale by a per-pop `reset` rather than named
byte-by-byte. Nothing else on this page applies there: the placement verbs and their
`Copy` guard are about values a *region* stores, while a scratch collection is a stack
local whose elements drop normally when it falls.

**A fill may allocate from the bump it is filling.** `slice_from_iter` reserves
its destination run before it computes the first element, so the iterator's own
per-element step is free to bump more out of the same region — the shape a caller
takes when each slot re-homes a name through `text`. A `Bump` never moves an
allocation, so the reserved run and whatever the step takes stay disjoint and a
nested allocation that needs a fresh chunk leaves the earlier one where it is.
That is what lets a caller build a run of region-resident elements without an
owned staging run in between, and it is pinned by
`a_fill_may_allocate_from_the_bump_it_is_filling` in the
[audit slate](../observe/miri_slate.md).

**A frozen keyed index is a verb, not a relaxation of `value`.** A table header is
glue-free without being `Copy`: a `hashbrown` map owns its bucket array, so it has a
`Drop`. `frozen_table` is the verb that admits it, and it *builds* the table rather
than placing one handed in. That is what
makes the admission safe rather than merely asserted. Suppressing the header's
destructor is lossless on two conditions — the entries carry no glue, and the
bucket array is bump memory the region releases whole. The first is checked, by a
monomorphization-time `const { assert!(!needs_drop::<K>() && !needs_drop::<V>()) }`
that fires at the declaration naming the entry types. The second *cannot* be
checked at a placement verb, because a table backed by the global heap and one
backed by this bump have the same type; building the buckets inside the verb is
what closes it, since no caller can supply a foreign-allocator table.

**`in_place` is the weaker verb, and deliberately so.** A value that keeps mutating
after it is stored cannot be `Copy` — copying it would fork the shared state — and
cannot be built inside a verb the way a frozen table is, because its interior tables
grow over the region's whole life rather than at one call. `in_place` therefore takes
the `!needs_drop` proof directly and closes only the first of the two conditions
above: it knows the value runs no destructor, and *cannot* know that a destructor it
is not running would have freed only bump bytes. That second half is the caller's,
discharged structurally rather than by signature — the admitted value's interior
tables are built over this same allocator, so the region releases them whole. It is
the one place the tier rests on a declaration site rather than a verb, which is why
it is a named verb with its own rationale instead of a `Copy` relaxation that would
admit every glue-free shape and its callers' assumptions with it. The one shape in
the tree that takes it is a scope: bump-resident, structurally `Drop`-free, and
mutating through `Cell` / `RefCell` for as long as its region lives.

The return is a plain `hashbrown` table (`BumpBackedMap`), not a veneer type: the
freeze is the shared reference, since no mutation is reachable through `&`. An
embedder that wants a table it keeps *writing* to builds one over the raw
`Allocator` seam instead and owes its own entry-glue assert at the declaration.
That seam stays the one place a non-`Copy` value can reach a region's bytes without
a verb's guard — it is what a collection is constructed over, and the embedder's
declaration site is what holds the line there.

The embedder's path in is two doors, split by whether the bumped value has
operands whose reach the product must carry.

[`FoldedPlacement::fold_and_bump`](../src/witnessed/bump.rs) is the reach-bearing
one, hung on the fold engines' placement capability so its brand `'b` is the
enclosing fold's, not one the door mints. It takes its operands as carriers,
composes and retains their reach into the destination *before* running the
caller's constructor, and hands back one bundled `Opened` — never a bare region
reference and never a `(value, reach)` pair, so reach stays a consequence of
which carriers were passed in rather than a claim a call site writes. The
constructor writes through a `BumpAllocator<'b>` over the destination region,
whose verbs are std shapes only — a `Copy` value, a `Copy` slice, a `str` —
which is what keeps the library free of any per-workload verb. A fold that already
holds a placement and has no operand reach left to compose reaches the same tier
directly through [`FoldedPlacement::allocator`](../src/witnessed.rs): it rests on
the identical brand argument and grants no more, with no audit and no `Option` —
the rank-2 brand discharges the residence obligation at compile time.

[`RegionHandle::allocator`](../src/witnessed/region.rs) is the handle-level door,
for a `Drop`-free value wanted at the handle's own frame lifetime rather than
confined to a fold closure. It has no operands and no reach to compose, so the fold
machinery has nothing to do and no call site can claim anything wrongly; what is
left is an ordinary borrow, the returned `&'a` against the `&'a Region` the handle
holds, which the borrow checker enforces with no audit and no `unsafe`. A value
built *around* those bytes that embeds a **foreign** operand is gated at its own
rank-2 brand (`bump_born_with` for a value built where it lands, `fold_and_bump` for
one built at a fold); one whose fields are all at this handle's own `'a` needs no
gate and takes `in_place` directly. Occupancy is one
whole-region figure,
[`Region::bump_capacity`](../src/witnessed/region.rs) — the allocator's
**reserved chunk capacity**, padding and the newest chunk's unused tail
included. A chunk's floor under a small region is the honest price, because a
pin retains chunks whole — and that floor is a whole frame wide: a fresh region
asks its bump up front for enough to hold a frame's entire residency
(`FIRST_CHUNK_BYTES`), so the common case is one chunk taken at the mint rather
than a doubling ladder climbed as the frame fills. Reading the figure off the allocator rather than
tallying it at the doors means an allocation that reaches the bump without a
door call — a collection built over it through `allocator-api2` — is priced like
any other. There is no per-family breakdown, because the copy-versus-pin
decision reads a region's total against a candidate value's own copy size and
never needs one.

The allocation *capability* is a distinct type from the region. The region's own
`allocator` is `pub(crate)`, so a bare `&Region` has no allocation
surface at all; the only public minter is
[`RegionHandle::from_owner`](../src/witnessed/region.rs), gated on the
unsafe-to-implement `RegionOwner` contract. An embedder that holds a region owner
picks up the library's region behaviour through that seam and allocates through
handles it threads itself.

## Construction: `yoke`, `merge_into`, `map`

The carrier `Witnessed<T, W>` bundles `Erased<T>` with the witness `W` that pins
it, so "the witness keeps the value alive" is a type invariant, not a co-stored
pair plus a comment. Three constructors build it; their division of labour is the
heart of the design.

**`yoke` — mint a value into a region.** `yoke` hands the witness's own region to
a rank-2 `for<'b> FnOnce(&'b Region) -> T::At<'b>` closure and bundles whatever it
builds. Because the closure is universally quantified in `'b`, it cannot return a
reference captured from its environment (a foreign `&'x` would need `'x: 'b` for
every `'b`) — so the produced value's references are *region-derived or owned*,
and co-location (the witness pins *this* value's references) holds by construction
rather than by assertion. The witness enters here, as a parameter, because there
is no prior carrier to inherit it from: `yoke` is the door through which a value
first becomes witnessed.

The witness `yoke` takes is a *single-region* type — a lone region owner — so a
mint pins exactly one region by construction, not by narrowing a set that might be
empty or hold several. A minted leaf lifts to the **reference-only** carrier an
aggregate stores through a distinct [`into_reference_only`](../src/witnessed.rs)
lift: its own region is kept alive externally, by containment or a holder's
owned pins, so that carrier holds no pin. Keeping the lift separate from `yoke` is what keeps
minting a one-region act, leaving the combining of regions to the merge.

**`merge_into` — fold many region-resident values into one.** A value built from
references into *two* regions cannot be bundled with one witness by `yoke` alone.
[`Delivered::merge_into`](../src/witnessed/delivered.rs) re-anchors two delivery
envelopes at one shared brand, runs a projection that binds one into the other,
and re-seals under the **composed** witness — the union of the two operands'
regions, with `outer`-chain subsumption dropping a region another already pins.
Neither operand's backing needs a pin threaded in from the caller: each envelope
carries its own pins, and the union of the two covers the shared re-anchor for
the whole fold. The composition is `ComposeWitness::compose`, run inside the
shared brand with the destination in scope: an owned region *set* composes by
plain union (total, since a set can always represent the combined pin), while a
hosted carrier mints the union into the destination's own arena. `merge_into` and
`transfer_into` run one crate-private engine, `Witnessed::merge_composed`, whose
`fold` builds the product and composes the witness inside a single brand. An
engine is the only thing that takes the pin, and each is reachable only from the
envelope verbs that derive it — the N-ary relocation below has its own,
`merge_staged_composed`, differing only in how many sources it re-anchors at once.

This is what keeps witnessed-ness at the *boundary*. Without it, an aggregate of
independently-witnessed elements would nest `Witnessed<…Witnessed<…>>` wrappers
with the data and be unstorable as a single cell carrier. With it, the invariant
holds:

> **One wrapper per cell.** A cell stores exactly one carrier, regardless of value
> complexity. `yoke` mints leaves into a region; `merge_into` folds
> region-resident values — same-region or cross-region — into one aggregate under
> the single witness that pins them all; the result seals as one unit. Wrapper
> count is O(1) per cell, not O(data size).

The merge's trigger is *referencing a pre-existing region-resident value* —
the foreign borrow a `yoke` closure would reject. Where the two operands are
ancestry-related, subsumption collapses the union to the deeper owner. The case
`merge_into` *cannot* collapse — a value whose backing reaches an **independent,
dying** region — is `transfer_into` (below) instead: there the source is a dying
*descendant*, so subsumption would collapse onto the backing about to drop, and
the union must be held *whole* as the set of both.

**`map` — advance a value already witnessed.** `map` consumes a carrier,
re-anchors it at a brand, transforms `T::At<'b> → P::At<'b>`, and re-seals under
the *same* witness. It differs from `yoke` in source (an existing carrier, not a
region) and from `open` in that the brand-flavoured result is *kept* — re-sealed —
rather than forbidden from escaping. It is how a witnessed continuation steps to
its next witnessed state without changing which region pins it.

## Storage and access: `seal`, `open`, `transfer_into`

A cell holds its carrier *between* run-loop steps, when nothing is being read. The
access surface models exactly that rhythm.

**Sealing.** `seal` turns the live `Witnessed<T, W>` into a `Sealed<T, W>` — the
cell-storage form, opaque between accesses, exposing no construction or transform.
Sealing is the same operation that lifts a finalized result into a slot: bundle
the erased value with the witness that pins it.

**Opening.** `open` is the access verb — a rank-2
`open<R>(&self, for<'b> FnOnce(Live<'b, T>) -> R) -> R`. Between calls the carrier
is `Erased`: no live reference exists. Each `open` is a borrow-scoped window in
which references go live, branded `'b`; `R` cannot name `'b`, so nothing branded
escapes the window. This is the design's safety core, and the RAII analogy is
exact: *behave like RAII while accessed — borrow-checked, references confined —
but instead of dropping, go opaque until the next access.* No `'b`, no access; a
value that must outlive the window leaves it only as an owned copy or by transfer.

**Transfer.** `transfer_into` is the safe relocation — it moves the sealed value
into a *consumer's* storage at the destination's lifetime, keeping every region
the value still reaches alive by holding that region's owner. Copying is not an
option in general: a captured closure may reference anything reachable from its
scope, and a region carries no per-value reachability map, so the source regions
are *kept*, not rebuilt. The carrier is therefore witnessed by the **set** of
regions the value reaches — the destination it was relocated into, plus each
source region a retained value still borrows. These regions form a tree, not a
chain — a closure capturing closures branches into independent lineages —
flattened into the set; a value with no cross-region reference is the degenerate
singleton. This closes the one case `open` cannot: a value whose source backing is
dying but whose consumer outlives it.

`transfer_into` shares its `ComposeWitness::compose` engine with `merge_into`,
so a relocated operand and a co-located one fold through one composition rule.

**Relocating a run.** A site building *one* value out of *many* delivered ones —
an aggregate literal's cells, a record's fields, a step's dep views — takes
`transfer_all_into`, the N-ary door over the same rule. It is not sugar over a
loop of `transfer_into`, and the reason is asymptotic: folding pairwise makes
each step's product the next step's destination, so the accumulator must carry
the run gathered so far, and it can only carry it as region-bumped bytes (the
destination family rests glue-free between steps, and each step's brand is
fresh, so no buffer named outside a step can receive a value built inside one).
The run is then re-bumped per step — quadratic in region bytes none of which a
bump can reclaim before its frame dies.

The N-ary door removes the accumulator instead of optimizing it. The sources'
erased forms are staged as one slice of the `Staged<S>` run family — a slice of
a layout-invariant family is itself layout-invariant, so it re-anchors through
the *same single retype* one operand takes — and `merge_staged_composed`
re-anchors that whole run together with the destination operand inside one
brand. The relocate hook therefore sees every source live at once and bumps the
product run exactly once, so region bytes and heap copies are linear in N.

Two things carry the run's obligations:

- **The staging pin.** The re-anchor needs coverage of every staged source's
  backing, not one. The caller's own borrowed slice of the sources' `PinBundle`s
  *is* that coverage, so `[&PinBundle<F>]` is itself a `Witness` and the engine's
  pin parameter is `?Sized` — nothing is unioned to present a pin the sources
  already hold between them.
- **Retention, asked per source.** The relocate hook returns the product
  together with the run of cells it built, and the door zips that run against the
  source envelopes to ask the retention predicate `(source, its own cell,
  region)`. That preserves `transfer_into`'s exact per-source answer rather than
  approximating it with a run-wide union, and it means no embedder-facing
  signature carries an index into a run it would have to trust. The residual
  contract is the hook's: cells come back in staging order, checked for length
  by a debug assert. The surviving members compose to a single antichain in the
  same walk that filters them (`PinBundle::union_all_retained`), which is what
  keeps the door's fixed cost at small N at or below the pairwise path's.

`transfer_into` stays the door for genuine 1:1 relocations — a seam crossing, a
`catch` arm — where there is no run to stage.

## The dormant slot and the two resting tiers

A dormant carrier's value is not live: nothing may be assumed about it until a
witnessed re-anchor. A struct field typed as a reference tells the abstract
machine the opposite. A function-entry retag descends into by-value aggregate
arguments and *protects* every reference it finds, so a carrier passed by value
while its own pins hold the last owner of the region its contents point into
would deallocate memory carrying a protected tag when those pins drop inside the
call. Retag does not descend into unions, so the slot is one:
[`Dormant<V>`](../src/witnessed/dormant.rs), a one-field union over
`ManuallyDrop<V>`. `Erased<T>` stores its `T::At<'static>` there, and so does the
carrier's own erased reach reference — which is itself an `Erased`, so both
protected tags go at once. The union carries no `repr`: nothing depends on its
layout, because every retype operates on a value moved *out* of the slot, never
on the slot itself. The declaring module is the single audited home for union
reads, and every one of them leans on one invariant — the slot is always
initialized: the only constructor initializes the field, no method deinitializes
it without consuming the wrapper, and the union has no other field.

A union field has no drop glue, and `Copy + Drop` cannot share a type, so the
resting surface splits into two tiers, partitioned by the marker trait
[`DropFree`](../src/witnessed.rs).

The **Copy tier** — `Erased`, `Sealed`, `SealedExtern`, and the `Witnessed` /
`Delivered` carriers built over `Erased` — bounds its family `T: DropFree` and
stores a bare `Dormant`, so resting a droppable family there is an ordinary
trait error at type-check time. `DropFree` is *safe* to implement: a false impl
leaks the value's owned contents (the glue-free slot runs no destructor) and
cannot cause UB. The intended route is the default
[`reattachable!`](../src/witnessed.rs) arm, which certifies the marker and backs
that certification with a `needs_drop` const assert — declaring a droppable
family through it is a compile error, not a silent leak. A family that genuinely
needs drop takes the macro's `droppable` arm, which emits the `Reattachable`
impl alone.

The **owned tier** is [`SealedPinned<T, W>`](../src/witnessed/dormant.rs): the
same union wrapped in the one type that owns the value's destructor, paired with
the pins covering the value, co-located at its erase door. Co-location at the
door is what makes the tier strict — a droppable erased value never exists
without its glue and its pins, unwind included — and field order (value before
pins) is what makes dropping one unopened sound: struct fields drop in
declaration order, so the value's own destructor may freely dereference region
memory the bundled pins still hold. Droppable *and* region-pointing families are
thereby supported rather than excluded. The bundled pin closes no reference
cycle: sections are `Copy` and `Drop`-free
([sectioned-reach.md](sectioned-reach.md)), so a region owns no counted edge back
out of itself.

`SealedPinned` ships a **single** consuming open verb, which re-anchors the
pinned value *and* a zipped `SealedExtern` operand at one brand under a
caller-supplied operand pin — the step-open shape, where an embedder's
continuation opens beside its scope operand and an invariant family rejects
separately-branded opens. There is no operand-free open: a caller with nothing
to zip passes a trivial operand, which keeps the surface to one verb.

The open's brand carries an **upper bound**. Its closure receives a
[`Within<'b, 'outer>`](../src/witnessed/dormant.rs) token — a ZST whose declared
`'outer: 'b` the `for<'b>` instantiation must discharge — so `'b` can no longer
be instantiated at `'static` behind the caller's back. This is
`std::thread::scope`'s shape (`for<'scope>` bounded by `'env` through the
`Scope<'scope, 'env: 'scope>` argument type), and it is the channel for an
**ambient capability** the embedder holds as a live borrow-checked reference for
all of `'outer`: a covariant value of that kind shortens to the brand by ordinary
subtyping inside the closure, and an invariant one rides at `'outer` inside a
brand-scoped struct that declares the same `'outer: 'b` bound the token
discharges. Either way it needs no seal, no re-anchor and no pin, because its
liveness is the borrow checker's rather than the witness system's. A value
whose lifetime *was* erased still enters through the sealed operand. The
anti-escape guarantee is untouched — the bound points one way, so nothing
`'b`-branded gains a route into the result or into `'outer`-typed storage, and an
invariant family still cannot unify `'b` with anything.

### What a droppable family accepts

The `droppable` arm is a narrower contract than the default one, not the same
declaration with a marker left off. Three things come with it.

**The value is opened once, by move.** The tier's open takes `self`: the erased
value leaves the slot, re-anchors, and is consumed inside the brand. The Copy
tier's `&self` reads — which copy the erased form out before re-anchoring it —
have no counterpart here, because a value that owns its contents cannot be
copied out of a slot that still holds it. A one-shot `Box<dyn FnOnce>` is the
tier's native shape: it neither has nor needs a `Copy` erased form, and its
single consumption is the single open.

**Coverage is dated from the erase, not from the open.** A Copy-tier witness has
to hold only while a re-anchored reference is live, because nothing else ever
touches the resting value. A droppable one is touched again at teardown: a
carrier dropped unopened still runs the value's glue, and that glue dereferences
whatever the value's contents point at. The pins bundled at the erase door
therefore have to cover every region the value reads for its whole dormant life.
Choosing that witness is thereby a claim about what the value holds — a
droppable family may hold what its pin transitively keeps alive and nothing
else, and the difference shows up at a teardown the read path never reaches.

**Layout invariance stays the family's to argue.** The `droppable` arm relaxes
`DropFree` alone; the `Reattachable` obligation is unchanged. A boxed trait
object generic only in `'r` discharges it exactly as a plain reference family
does — a fat pointer's layout is identical for every choice of the lifetime —
which is what lets an owning one-shot closure be a family at all.

The scheduler's node slot is the tier's production instance. `Workload::Continuation`
is `Reattachable` alone — no `DropFree` — because the slot rests it as
`SealedPinned<W::Continuation, Rc<W::Frame>>`
([nodes.rs](../src/scheduler/nodes.rs)): a one-shot boxed closure owning its
captures. An embedder hands an install path a `NodeWork<'a, W>` holding the
continuation **live** at its own lifetime, and the scheduler's single private
erase door seals it against that install path's *effective* anchor `Rc`, storing
the result as the scheduler-internal `StoredWork<W>`. Because the anchor
transitively holds the storage chain the continuation reads, the bundled pin is
the same liveness the step open was already bounded by — now carried rather than
supplied externally, so a parked slot torn down unopened runs the continuation's
glue while the memory that glue touches is still pinned. No embedder call site
ever pairs a continuation with a pin by hand.

## Three witness forms

A sealed carrier comes in three shapes, distinguished by where the witness lives.

- The **self-witnessed** form bundles `W` (the `Sealed<T, W>` above): for a value
  *minted* into a region whose pin nothing else holds. `yoke`, which moves `W`
  into the bundle, builds this form.
- The **externally-witnessed** form carries *no* bundled witness; the holder
  already pins the backing and supplies it at the access, read through a
  **consuming, externally-witnessed `open`** — the witness handed in at the call
  and the carrier moved into the same rank-2 `for<'b>` brand, so a non-`Copy`
  carrier passes and nothing branded escapes. It is built with the witness-less
  `erase` and read against an external pin. It rests in the Copy tier, so its
  family is `DropFree`.
- The **internally-witnessed** form (`SealedPinned`, the owned tier above) is
  where a *droppable* family rests: the pins are bundled, like the
  self-witnessed form, but they are bundled at the erase door rather than
  inherited from a construction verb, because the value's drop glue is what they
  have to outlast.

Bundling a witness the carrier does not need would be a redundant second owner —
and, when the witness is reference-counted, an extra count the holder's own
uniqueness checks must subtract. A value held *inside* the region it would witness
therefore takes the externally-witnessed form; a value held *outside* that region
takes the self-witnessed one. A droppable value bundles regardless of where it is
held: its destructor runs at a moment no external holder is present to supply a
pin for, so the pin has to be its own.

The split is what keeps self-witnessing cycle-free. A self-witnessed carrier's
strong region owner rides the *carrier*, which a cell holds outside the region it
witnesses; `merge_into` folds every intermediate into that one carrier (the *one
wrapper per cell* invariant), so no region-resident value strong-owns its own
region — the value in-region holds only non-owning pointers. A value that
*captures* an in-region value has no bundled witness to merge against: it
mints its merge operand from the region owner its builder already holds, so the
capturing carrier gains that owner and pins the region exactly as a cell result
does.

## Why reads are safe

The danger in any reattach is a *free, unbounded* content lifetime the caller can
widen past the witness pin — a use-after-free the naive content-free reattach
exhibits. `open`'s rank-2 brand forecloses it: the fabricated lifetime is
universally quantified and un-nameable, so it cannot be widened or captured. Reads
therefore lose no safety — a reference may escape the *call* (the value drives the
step's work), but only as an owned copy or pin-bounded transfer, never as a
branded borrow outliving its window.

The cost the brand imposes is structural: it forces the entire per-step
consumption to nest inside the closure. Where a re-anchored reference would
otherwise ride up the caller's stack, that becomes either copy-out or a CPS
rewrite of the step. An embedder's run loop nests its whole step tail this way —
continuation run, outcome apply, and finalize all inside one brand — so nothing
branded crosses the step boundary, and `open` is the substrate's entire access
surface: there is no borrow-bounded accessor beside it.

## Storage choice belongs to the workload

The substrate is parametric over the witness `W`, and assumes nothing about which
storage backs a given carrier. A carrier may witness a freshly-allocated region or
borrow storage its creator already holds; the substrate routes both through the
same surface. Whether a given cell installs a fresh region or allocates into an
ambient one, and whether a chain of cells reuses one region or turns it over, is
the workload's call — which is why "per-cell memory" names the carrier a cell
holds, not an arena the cell owns.

Two allocation modes ride that parametricity, sharing one carrier and reach type.
Inside a step, the step construction context is the maximally-checked path: the
scheduler holds the consumer's region owner for the step's duration, so region
access is infallible and every allocation is brand-confined. Outside a step, an
embedder allocates through a held [`RegionHandle`](../src/witnessed/region.rs) —
the `yoke` / `merge_into` surface above — with the same guarantees and no
scheduler involvement.
