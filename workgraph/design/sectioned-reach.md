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

The region's description side table ([`Region::alloc_reach`]) is an
intern table: minting looks up the composed member set and allocates
only on a miss.

- **Key.** The canonical member set: member owner addresses, sorted.
  Well-defined on two grounds. The antichain is unique *as a set* —
  [`PinBundle::insert`]'s outer-chain subsumption normalizes to the
  deepest owners regardless of fold order; only the `Vec` order varies.
  And addresses are stable — an entry's `Weak` members keep each
  allocation alive through the weak count, so no reuse while the entry
  exists. The host never enters the key: every description in a table
  shares the table's own region owner as host.
- **Contract unchanged.** [`ReachDescription::mint`] composes the
  antichain from its sources as before, then interns. Callers still
  receive `(&'a description, PinBundle)`; on a hit the reference names
  an existing entry with the same `'a`. Interning dedupes the
  description object, not any holder's coverage — a caller that keeps
  its own bundle (a delivery envelope, an embedder's own holder) still
  composes it.
- **Region-lifetime retention dedupes fully.** On a hit, the region's
  union bundle already pins the entry's members from its first mint,
  and every holder of the interned reference lives inside that region,
  so the [`Region::retain_reach`] fold is skipped. One description and
  one region-lifetime pin fold per distinct reach per region, ever.
- **The empty description** (reaches nothing; residence only) is a
  per-region interned singleton shared by every region-pure value and
  every owned-data run.

## Sectioned storage

A payload-generic container type: cells in semantic order, physically
partitioned into contiguous **runs**, each run pairing a span of cells
with one interned `&ReachDescription`. The payload type is the
embedder's; embedder values never enter workgraph.

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
- **Confinement.** A projected cell is `'a`-confined to its
  container's region through both the payload and the run's
  description reference; it cannot outlive the container without
  passing a mint-consuming seam that relocates its reach into a
  destination. The compiler enforces the seam.

## The alloc door

Sectioned containers are built through one door: constructor plus
per-input `(payload, envelope, copy-or-pin verdict)`, producing
sectioned storage in the destination region.

- **Copied input**: rebuilt at the destination carrying each
  surviving sub-run's reach. A fully-owned input lands in an
  empty-reach run with no walk — its own runs already say so.
- **Pinned input**: the run's description is get-or-minted from the
  input's members plus the input's home region as an ordinary member;
  the owning pins fold into the destination's union bundle (skipped on
  an intern hit).
- **The container's value-level description** is the get-or-mint of
  the union over its run descriptions — cheap under interning, so
  whole-value carriers keep their single stored `&ReachDescription`
  shape unchanged.

The embedder supplies the copy-or-pin cost predicate, the deep-copy
hooks, and the born-borrowing seeds (values whose description is
declared at construction rather than composed from the door's inputs).
Everything else — grouping into runs, interning, pin folding, the
value-level union — is workgraph's.

## What this replaces

With reach stored per run and derivation behind the alloc door, the
embedder's seam-time shape walks retire: escape probes
(does-this-still-borrow-the-source), release-exact subset derivation
at projection, and residence audit walks. A transfer claims the empty
source bundle exactly when no surviving run names the source region —
a stored fact, not a probe. The embedder's reach obligation shrinks to
the per-input verdicts and the born-borrowing seeds.
