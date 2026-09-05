# Retention pricing

**Problem.** The substrate offers two ways to keep a value alive across a
cell's death — copy it out, or hold the region — and three merges that trade
storage for records, but nothing prices them. The cost of holding a sealed
region is the storage of its aggregate's transitive closure; releases whose
closures overlap double-bill the shared part; a loop cart that absorbs each
retiring predecessor accretes dead bytes while one that copies pays per hop
([liveness-matrix.md § Bounding the two tiers](../design/liveness-matrix.md#bounding-the-two-tiers)).
An embedder deciding copy versus hold, absorb versus copy, or persistent cart
versus turnover has no number to decide with.

**Acceptance criteria.**

- The table answers, for a sealed id, the storage size of its aggregate's
  transitive closure, memoized once every slab bit in the closure has
  converted and invalidated never.
- The table answers, for a set of candidate releases, the uniquely-held
  slice of each candidate's closure, so the marginal price of one release is
  not double-billed against another's.
- The table answers, for a live cell, its region's current size and the size
  of storage absorbed into it since a given mark, so a loop cart's accreted
  dead bytes are measurable without a scan.
- A pressure signal exposes tier occupancy — live slots used, sealed bytes
  retained — so an embedder can ramp a copy-versus-hold threshold ahead of
  the slab cap.
- None of these queries mutates a hold, and none is consulted on a mint or a
  release path inside the substrate itself.

**Directions.**

- *Where the choice is made — decided.* In the embedder. The substrate
  prices; the crossing rule, the cart shape, and the consolidation trigger
  are embedder policy over those prices.
- *Pressure model — open.* A linear ramp on occupancy versus a step function
  at fixed watermarks. Left to the first embedder's measurements.

## Dependencies

**Requires:** none — the merges it prices are in the substrate.

**Unblocks:** none tracked — the substrate's terminal item.
