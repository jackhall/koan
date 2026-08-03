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

### Two doors into a typed cell

A `FamilyArena` cell stores `K::At<'static>`, so *what* a store may admit is the whole
question of residence: a region borrow the erasure discards is one only the store's
signature can vet.

[`RegionHandle::alloc_resident`](../src/witnessed/region.rs) is the **move-in** door. Its
bound is `K::At<'static>` — the value carries no region borrow at all, so there is nothing
to vet and nothing a call site could claim wrongly.

[`RegionHandle::alloc_resident_born`](../src/witnessed/region.rs) is the **born** door, for
a family whose every constructor borrows the region it lands in and so can never meet that
bound. It takes a `for<'b>` construction closure and hands it a `FoldedPlacement` over this
handle's own region re-anchored to `'b`, then stores what the closure returns and gives back
the resident at the handle's `'a`. The proof is the quantifier: `'b` has no outlives relation
to any enclosing lifetime, so the only `&'b Region<W>` — and hence the only `X<'b>` built over
one — the closure body can name is the placement's. A captured ambient `&'a Region` does not
coerce and does not compile, which makes the built value's region pointer the destination's by
construction. It is the same no-outlives argument
[`FoldedPlacement::alloc_resident_folded`](../src/witnessed/bump.rs) rests on, for a value
with no fold to ride.

`alloc_resident_born_with` is the born door for a value that must embed an operand borrowed
from *outside* the closure. The operand arrives as a `SealedExtern` and is re-anchored to the
*same* `'b` as the placement — one `zip`ped `open`, because branding the two independently is
exactly what an invariant family rejects. What the signature still cannot prove is the
operand's own liveness: its pointee may live in another region, and the stored value keeps
naming it for as long as the destination region lives. The `pin` argument is where a caller
discharges that, and it is borrowed for `'a` — the destination region's own lifetime — so the
`Witness` contract covers the stored reference's whole life rather than merely the call. That
keeps the door a lifetime *shortening*; the residual co-location obligation (does this pin in
fact cover this operand?) is the one every `Witness` already carries, narrowed in duration
rather than added to.

## The bump allocator

`Region<P>` is the erase-store engine: a set of typed sub-arenas parameterized by
a storage profile `P`, holding `At<'static>` and handing back an `&'a` tied to
the caller's input borrow. A workload declares only its family list (`FamilyList`,
a `(K, Rest)` cons-list) and the library derives and owns the arena bundle from
it — one `FamilyArena` cell per family, keyed by `Stored::cell` through a
tuple-field path, so a wrong binding is a compile error rather than a runtime bug.

The region keeps the typed-arena Drop discipline — each stored value's `Drop`
runs, and touches only owned contents, never a lifetime-parameterized reference
(sub-arenas drop together, so any cross-arena `&` is dead before it could be
observed). This is what makes a byte-bump allocator that forgoes Drop (`bumpalo`)
the wrong fit *for the typed families*: their Drop discipline *is* the soundness
argument, and dropping it would mean re-proving every stored type leak-free by
hand.

Beside those cells a region holds a **bump**, and it is a second storage tier
rather than a private scratch area: it is the home for any `Drop`-free value
family — the library's own container metadata (a sectioned container's run
partition and cell index block, see
[sectioned-reach.md](sectioned-reach.md)) *and* an embedder's value families,
which reach it through one door. Two properties define the tier.

**It routes no erasure.** The allocator's own type carries no lifetime, so `'a`
enters only at the allocating call. A bumped value may therefore hold an `&'a`
back into the very region it lives in, with no `erase_to_static`, no `Stored`
impl and no residence check — the thing a `FamilyArena` cell
cannot do, because its `K::At<'static>` slot type forces a region-self-referential
value's borrow through erasure and back through a brand-confined reattach. Cycles among
bumped entries are harmless: everything there dies with the region, at once.

**`Copy` is the static proxy for "no destructor to skip".** A bump releases its
chunks whole and runs nothing, so a `Drop`-bearing entry would silently leak what
it owns. "`Drop`-free" has no expressible bound, so every placement primitive
carries `T: Copy` instead — the honest approximation, and the bound that keeps a
sectioned container `Copy` and free at region teardown.

One primitive stores a value that is not itself `Copy`, and it is admitted by
moving the bound rather than by dropping it.
[`BumpMap`](../src/witnessed/bump.rs) is a `hashbrown` table for an embedder's
keyed index — the shape a container reaches for when a sorted slice and a binary
search will not do — built over the region's bump through `allocator-api2` and then
placed in that same bump. Its `K: Copy` / `V: Copy` bounds sit on the type's own
inherent impl, covering construction and every read, and they are what make the
un-run destructor lossless: with `Copy` elements the table's `Drop` has nothing to
do *but* free the bucket array, and that array lives in the very chunks region death
releases. No `unsafe`, no `ManuallyDrop`. The surface is read-only — a table is
frozen at construction because the value it indexes is, and mutation would have to
rehash into the bump, stranding the old bucket array as garbage no occupancy figure
could account for honestly.

The embedder's path in is two doors, split by whether the bumped value has
operands whose reach the product must carry.

[`FoldedPlacement::fold_and_bump`](../src/witnessed/bump.rs) is the reach-bearing
one, hung on the fold engines' placement capability so its brand `'b` is the
enclosing fold's, not one the door mints. It takes its operands as carriers,
composes and retains their reach into the destination *before* running the
caller's constructor, and hands back one bundled `Opened` — never a bare region
reference and never a `(value, reach)` pair, so reach stays a consequence of
which carriers were passed in rather than a claim a call site writes. The
constructor writes through a `BumpPlacement`, minted only inside a door call,
whose primitives are std shapes only — a `Copy` value, a `Copy` slice, a `str` —
which is what keeps the library free of any per-workload verb. A fold that already
holds a placement reaches the same tier directly through
[`FoldedPlacement::bump`](../src/witnessed.rs), the `Drop`-free peer of
`alloc_resident_folded`: it rests on the identical brand argument and grants no
more, dropping only the `Stored` requirement and with it the erase/re-anchor round
trip a bump needs no part of.

[`RegionHandle::bump_text`](../src/witnessed/region.rs) and its siblings
(`bump_value`, `bump_slice`, `bump_map`) are the handle-level ones, for a
`Drop`-free value wanted at the handle's own frame lifetime rather than confined to
a fold closure. They have no operands and no reach to compose, so the fold
machinery has nothing to do and no call site can claim anything wrongly; what is
left is an ordinary borrow, the returned `&'a` against the `&'a Region` the handle
holds, which the borrow checker enforces with no audit and no `unsafe`. Storing a
value into a *typed* family is gated at its own door — `alloc_resident`'s
`'static` bound for a value that borrows no region, or one of the two rank-2 brands
(`alloc_resident_born` / `alloc_resident_born_with` for a value built where it lands,
`alloc_resident_folded` for one built at a fold) — none of which these doors touch. Occupancy is one
whole-region figure,
[`Region::bump_bytes`](../src/witnessed/region.rs) — **live bytes**, summed over
what each allocating call actually stored, not the allocator's reserved chunk
capacity, which would put a whole chunk's floor under a small region. There is no
per-family breakdown, because the copy-versus-pin decision reads a region's total
against a candidate value's own copy size and never needs one.

The allocation *capability* is a distinct type from the region. The engine's
`alloc_resident` is `pub(crate)`, so a bare `&Region` has no allocation
surface at all; the only public minter is
[`RegionHandle::from_owner`](../src/witnessed/region.rs), gated on the
unsafe-to-implement `RegionOwner` contract. An embedder that holds a region owner
picks up the library's region behaviour through that seam and allocates through
handles it threads itself.

## Construction: `yoke`, `merge_pinned`, `map`

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
lift: its own region is kept alive externally, by containment or a retention hold,
so that carrier holds no pin. Keeping the lift separate from `yoke` is what keeps
minting a one-region act, leaving the combining of regions to `merge_pinned`.

**`merge_pinned` — fold many region-resident values into one.** A value built from
references into *two* regions cannot be bundled with one witness by `yoke` alone.
`merge_pinned` re-anchors two carriers at one shared brand under an **externally
supplied pin** covering the source (`self`) operand's backing, runs a projection
that binds one into the other, and re-seals under the **composed** witness — the
union of the two operands' regions, with `outer`-chain subsumption dropping a
region another already pins. The composition is `ComposeWitness::compose`, run
inside the shared brand with the destination in scope: an owned region *set*
composes by plain union (total, since a set can always represent the combined
pin), while a hosted carrier mints the union into the destination's own arena.

This is what keeps witnessed-ness at the *boundary*. Without it, an aggregate of
independently-witnessed elements would nest `Witnessed<…Witnessed<…>>` wrappers
with the data and be unstorable as a single cell carrier. With it, the invariant
holds:

> **One wrapper per cell.** A cell stores exactly one carrier, regardless of value
> complexity. `yoke` mints leaves into a region; `merge_pinned` folds
> region-resident values — same-region or cross-region — into one aggregate under
> the single witness that pins them all; the result seals as one unit. Wrapper
> count is O(1) per cell, not O(data size).

`merge_pinned`'s trigger is *referencing a pre-existing region-resident value* —
the foreign borrow a `yoke` closure would reject. Where the two operands are
ancestry-related, subsumption collapses the union to the deeper owner. The case
`merge_pinned` *cannot* collapse — a value whose backing reaches an **independent,
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

`transfer_into` shares its `ComposeWitness::compose` engine with `merge_pinned`,
so a delivered operand and a co-located one fold through one composition rule.

## Two witness forms

A sealed carrier comes in two shapes, distinguished by where the witness lives.

- The **self-witnessed** form bundles `W` (the `Sealed<T, W>` above): for a value
  *minted* into a region whose pin nothing else holds. `yoke`, which moves `W`
  into the bundle, builds this form.
- The **externally-witnessed** form carries *no* bundled witness; the holder
  already pins the backing and supplies it at the access, read through a
  **consuming, externally-witnessed `open`** — the witness handed in at the call
  and the carrier moved into the same rank-2 `for<'b>` brand, so a non-`Copy`
  carrier (a continuation) passes and nothing branded escapes. It is built with
  the witness-less `erase` and read against an external pin.

Bundling a witness the carrier does not need would be a redundant second owner —
and, when the witness is reference-counted, an extra count the holder's own
uniqueness checks must subtract. A value held *inside* the region it would witness
therefore takes the externally-witnessed form; a value held *outside* that region
takes the self-witnessed one.

The split is what keeps self-witnessing cycle-free. A self-witnessed carrier's
strong region owner rides the *carrier*, which a cell holds outside the region it
witnesses; `merge_pinned` folds every intermediate into that one carrier (the *one
wrapper per cell* invariant), so no region-resident value strong-owns its own
region — the value in-region holds only non-owning pointers. A value that
*captures* an in-region value has no bundled witness to `merge_pinned` against: it
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
the `yoke` / `merge_pinned` surface above — with the same guarantees and no
scheduler involvement.
