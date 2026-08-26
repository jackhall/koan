# Sectioned reach

Reach evidence stored at sub-value granularity. Two pieces, designed
together: the region's reach side table interns descriptions
(get-or-mint keyed on the member set), and container storage is
physically partitioned into contiguous runs that each pair a span of
cells with one interned description. Together they make a cell's reach
a stored, O(1)-readable fact at every granularity, so any seam that
parts a cell from its container reads reach instead of re-deriving it
by walking the value.

## Interned side table

The region's description side table
([`Region::intern_reach_retained`](../src/witnessed/region.rs)) is an
intern table: minting looks up the composed member set, allocates only on
a miss, and folds the region's retention on that same miss.

- **Key.** The canonical member set: member owner addresses, sorted.
  Well-defined on two grounds. The antichain is unique *as a set* —
  [`PinBundle::insert`]'s outer-chain subsumption normalizes to the
  deepest owners regardless of fold order; only the `Vec` order varies.
  And addresses are stable — an entry's `Weak` members keep each
  allocation alive through the weak count, so no reuse while the entry
  exists. The host never enters the key: every description in a table
  shares the table's own region owner as host.
- **The mint's contract.** [`ReachDescription::mint_resident`] composes
  the antichain from its sources, then interns; a caller receives the
  `&'a description` alone, which on a hit names an existing entry with
  the same `'a`. Interning dedupes the description object, not any
  holder's coverage — a value that travels on takes a transit copy of the
  composed pins ([reach.md § Threading](reach.md#threading-how-pins-reach-each-holder))
  on top of the destination's own retention.
- **Region-lifetime retention dedupes fully.** One description and one
  region-lifetime pin fold per distinct reach per region, ever, because
  the two are the *same act*: a miss interns the entry and folds the
  composed bundle into the region's union
  ([`Region::retain_reach`](../src/witnessed/region.rs)), a hit returns
  the entry and folds nothing. Every mint is a resident mint, so an
  entry exists in a table only because some earlier mint retained an
  identical member set there — the hit **is** the proof, and the table
  needs no side record of what the region already pins.
- **The empty description** (reaches nothing; residence only) is a
  per-region interned singleton shared by every region-pure value and
  every owned-data run.

## Sectioned storage

A cell-generic container type
([`Sectioned`](../src/witnessed/sectioned.rs)): cells in semantic order,
physically partitioned into contiguous **runs**, each run pairing a span
of cells with one interned `&ReachDescription`. The cell type is the
embedder's, named as a `Reattachable` family; embedder values never enter
workgraph. A container is immutable after the door — there is no push,
insert or remove — so a run's description can never drift out of
exactness with the cells it covers.

Cell *layout* stays the embedder's. A cell reaches the container already
resident, as the tight `&'a K::At<'a>` shape (content == borrow == `'a`)
a region allocation hands back, so what workgraph holds is the
index→cell mapping and the run partition, never the bytes. That is also
what anchors confinement: both halves of the container carry the
destination region's own `'a`, so one pin covers a cell and its reach
together.

A container is **`Copy` and `Drop`-free**: the mapping and the partition
are two slices bumped into the destination region, not heap buffers the
container owns. So a container is region state a holder *names*, and a
frame teardown releases it with the region rather than walking it — the
per-value drop work a `Drop`-bearing container would put back on the
teardown path. A bump rather than a lifetime-typed cell for two reasons:
the allocator is lifetime-free, so `'a` enters only at the allocation and a
run may hold an `&'a` back into the same region (a typed cell's own type
would have to name `'a`, which is why a `ReachDescription` is
lifetime-free instead); and a bump releases its chunks whole, running no
destructor — which is the point, and why only `Copy` data may go in.
Reference cycles among region-resident bumped entries are harmless: it
all dies at once. A container's two slices are one writer among several:
the same bump is the storage home for any `Drop`-free value family,
including an embedder's, reached through the door
[witnessed-memory.md § The bump allocator](witnessed-memory.md)
describes.

- **Exact per run.** A run's description is precisely the shared reach
  of its cells; adjacency decides sharing. Exactness is what makes
  projection release-exact: a cell parted from its container carries
  exactly its own reach, never the container's union. The same reach
  in non-adjacent runs makes two run entries naming one interned
  description.
- **Lookup.** The run covering index `i` by binary search over run
  starts. The single-run container — all-owned, or one shared reach —
  is the fast path: no per-cell cost, one description per container.
- **Degenerate interleaving** (alternating owned/borrowing cells)
  degrades to runs of length one — the per-cell-envelope cost floor,
  never worse than storing reach on every cell.
- **Projection is bundled.** Parting a cell hands back one value — an
  `Opened<'a, CellRef<K>, Carrier<F>>`: a reference to the cell paired
  with a carrier naming exactly that cell's run reach. Never a payload
  and a description as loose parts, for the reason the whole carrier
  model is: the value↔reach pairing is a type invariant rather than
  caller discipline. The cell rides as a *reference*, so parting costs no
  clone however expensive the cell is.
- **In-place reads are not a parting seam.** An embedder's traversals
  that never relocate a cell — equality, rendering, a copying rebuild —
  read the cells directly (`cells()`, `cell(index)`), the twin of the
  reach-only `reach_at(index)`: one yields cells and no reach, the other
  reach and no cell. Neither needs to be a seam, because both run under
  the pin the caller already holds (it holds the container) and a cell
  reference is `&'a K::At<'a>`, confined to the container's own region by
  its type. Only `project` pairs the two, and it is the only route by
  which a cell can travel.
- **Confinement.** The `Opened` state carries the container's region
  lifetime `'a` through both halves — the cell reference is
  `&'a K::At<'a>`, and the carrier's description is hosted in that same
  region — so a projection outliving the region is a compile error. It is
  deliberately *not* a `Sealed`: a seal is lifetime-free by construction
  (that is what makes it the dormant storage form) and would outlive the
  region freely, naming its coverage only at the open. A cell that
  genuinely travels passes the re-seal, which is the mint-consuming
  relocation seam. The compiler enforces the seam.
- **The relocation seam is one verb.** `Opened::lift_out` re-seals the
  projection and lifts it into a delivery envelope owning its whole
  reach: the run description's members upgraded `Weak → Rc`, plus the
  region the cell lives in — read off the description's own host, so no
  caller pairs a value with a residence it did not derive. The upgrade
  runs under the `'b` pin the open already borrows, which is the holder
  rule. The lifted envelope states exactly the run's reach; the
  container's union never enters, which is what makes a projection
  release-exact end to end
  ([reach.md § The carrier states](reach.md#the-carrier-states)).

## The alloc door

Sectioned containers are built through one door
([`Sectioned::build`](../src/witnessed/sectioned.rs)): the destination
region plus one `(resident cell, reach verdict, weight)` per cell.
Pairing them in one input value is what makes a fact-per-cell mismatch
unrepresentable — there is no cells sequence and verdicts sequence to
fall out of step. The unit is the **cell**, so a copied input whose own
runs differ arrives already expanded into per-cell verdicts, each
carrying that sub-run's surviving reach. Three verdicts, and no fourth:

- **Owned**: fully owned at the destination — a copied cell, or owned
  data. Lands in an empty-reach run with no walk.
- **Pinned**: the cell keeps borrowing its source. The run's description
  is get-or-minted from the input's *stored* description's members —
  which are exact, where the envelope coverage a step holds is generally
  wider — plus, under the run-level self rule below, the input's home
  region as an ordinary member; the owning pins fold into the
  destination's union bundle. The verdict carries the coverage pinning
  those members across the weak→strong upgrade: the holder rule,
  discharged at the door.
- **Seed**: a born-borrowing cell, its reach declared at construction
  from pins the caller already holds rather than composed from a stored
  description.

**The run-level self rule.** A `Pinned` cell's own residence joins its
members only when that residence is somewhere *other than* the
destination. A cell already resident in the destination is covered by the
destination's own liveness, so naming it would make every container
holding a co-resident sub-container read as borrowing its own home, and
the borrows-home memo folded from these runs would stop answering the
question it exists for: does a *borrow leaf* point home. A cell resident
elsewhere is a genuine cross-region borrow and its host folds in as an
ordinary member — nothing else would pin it. The rule is what makes an
embedder's borrows-home query exact rather than merely conservative, and
it is the run-level counterpart of the owned-bundle self rule
([reach.md § Composition](reach.md#composition-minting-a-description-and-retaining-its-pins)):
both say a region is never named against itself, one for pins, one for
members.

**The container's value-level description** is the get-or-mint over the
union of the per-cell reach sources — the same member set as the union
over its run descriptions, taken *before* the mint so that a cell
genuinely borrowing into the destination (a born-borrowing seed naming it,
or a foreign cell whose own description names it) keeps home in the
description, where the owned-bundle self rule would have stripped it from
a returned bundle. Cheap under interning, so whole-value carriers keep
their single stored `&ReachDescription` shape unchanged.

The embedder supplies the copy-or-pin cost predicate, the deep-copy
hooks, and the born-borrowing seeds. Everything else — grouping into
runs, interning, pin folding, the value-level union, the weight total —
is workgraph's.

**The door streams, and allocates nothing off the region.** Inputs arrive
as an exact-length iterator and are consumed one cell at a time, so an
embedder feeds its per-cell chain straight in rather than staging it in a
buffer of its own. Only the order *within* one cell is a law — whatever
the embedder must do before its verdict is exact, then the verdict, then
the store; across cells the run partition and the interning are
order-insensitive, so the embedder's per-cell work interleaving with the
door's own loop changes nothing either side observes. The exact length is
what lets the door's two working buffers reserve their region bytes once
up front: one cell slot per input, and runs at the same count, since a
run boundary costs a cell (runs ≤ cells). Neither buffer can grow, and
that bound is load-bearing rather than a tuning choice — the embedder is
bumping into the same region as it streams, so a buffer that grew
mid-build would abandon its reservation as dead region bytes. Those
reserved buffers *are* the container's stored slices, left in place at
the end rather than copied out, so the whole build touches no heap.

## Weight

Beside each cell's reach verdict the door takes its **weight**: a `u64`
whose meaning is entirely the embedder's — workgraph neither reads nor
interprets it. The door folds the cells' weights (saturating, so an
overflowing total reads as immense rather than small) into one container
total, stored beside the runs and read back through
[`Sectioned::weight`](../src/witnessed/sectioned.rs).

It rides here for the reason a run's description does: it is a fact about
the cells fixed at construction, and a container is immutable after the
door, so the total can never drift. An embedder that prices a container —
koan's copy-versus-pin decision is the motivating case
([value-substrates.md § Cost-driven copy](../../design/value-substrates.md#cost-driven-copy-the-optimization))
— reads the memo rather than folding over cells at a door of its own, and
a container nested as a cell contributes its own stored total in O(1). An
embedder that prices nothing hands in `0` and the total stays zero.

## What this replaces

With reach stored per run and derivation behind the alloc door, the
embedder's seam-time shape walks retire: escape probes
(does-this-still-borrow-the-source), release-exact subset derivation
at projection, and residence audit walks. A transfer claims the empty
source bundle exactly when no surviving run names the source region —
a stored fact, not a probe. The embedder's reach obligation shrinks to
the per-input verdicts and the born-borrowing seeds.
