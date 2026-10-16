# A tenant's own lend

A `Shares` frame borrowing its specialization through a lend of its own.

**Problem.** A frame born `Shares` is a tenant of its host and borrows through
it: cellgraph's `create_tenant` takes no `Lend`, a tenant's `'lent` is its
host's (`cellgraph/src/lend.rs`), and `seek` in `src/program/specialize.rs`
answers `None` for any placement but `Fresh`, so the drain reads no `Seek` for
a `Shares` birth. `placement_of` in `src/program/body.rs` places a callee
`Fresh` only where its return is `Number`, `Bool` or `Null`, so a body
returning a module, a list or a string is a tenant, and such a caching body's
warm tier is its host's scratch
([the scratch branch as a cache](../../design/type-tiers.md#the-scratch-branch-as-a-cache)):
its invariant types are warm across the calls of one park and re-laid at the
next, and nothing a binding settles for it has a build cell to land in.

**Acceptance criteria.**

- cellgraph creates a tenant with a `Lend` as `create_tree_lent` creates a
  tree cell: a tenant stores its own `Lent`, its step's `'lent` is that half,
  and a region
  lend other than its host's own is counted on the lender from the tenant's
  creation until its host's scratch next resets after the tenant's release,
  the host's scratch holding one count per lender.
- A keyed call born `Shares` hands the drain its `Seek`; the drain finds or
  builds the specialization as it does for a `Fresh` birth and creates the
  tenant lending its region; a tail hop of the same callee keeps the lend.
- A keyed `Shares` caching body runs on a reading chain over the lent branch
  and the host's scratch, its invariant sites interning into the lent branch
  and its exits adopting into the host's region.
- A test calls a keyed `Shares` caching body from two parks and finds the
  second's invariant types and verdicts laid by the first; a test evicts the
  specialization while a tenant borrows it, and the tenant runs to its end.
- A loop handing a module built over its link each hop to a keyed `Shares`
  caching helper holds adoptions flat per hop.
- The tenant lend's tests are on the Miri slate.

**Directions.**

- *Where the count drops* — decided. Where the host's scratch next resets
  after the tenant's release, or where the host disposes. A tenant step writes
  its host's scratch, whose header later steps of the host and its other
  tenants read, and `'lent: 'scratch` lets it leave a `'lent` reference
  there: a node a keyed body interns in the host's scratch branch references
  the specialization's nodes. Exit adoption covers the values that leave, not
  that branch. Dropping the count at the host's disposal instead would pin
  every specialization a long-lived host's tenants borrowed;
  [a tenant's own scratch](../../cellgraph/roadmap/tenant-scratch.md) would
  let it drop at the release.
- *Reaching the host's region* — decided. A scratch chain reads one tier
  between the root and its own, here the lent branch, so the host's region is
  read through the host's scratch branch's link, which today only a `Fresh`
  frame's own scratch has. A reading chain over a scratch branch with no link
  links it to its write home's region: laid linked where unlaid, linked late
  where laid, which the lattice allows a branch no other branch links.
  Rejected: linking every scratch branch as it is laid, which lays a branch at
  every park of ordinary code and a region branch at most; and a chain
  reading two tiers between the root and its own, a wider lattice change.
- *The placement bit* — decided. `placement_of` is untouched: a keyed body is
  not made `Fresh` to reach a specialization, since a `Fresh` result crosses
  into its home at the verdict's price, which the bit exists to avoid.

## Dependencies

The lend is cellgraph's [named lend](../../cellgraph/src/tree/README.md#death)
taken by a tenant, and the seek is
[the scheduler's](../../src/scheduler/README.md#specializations).

**Requires:**

- [Settled values](settled-values.md) — a keyed `Shares` body's invariant
  sites walk their operands into the lent branch on every call, affordable
  only past a settled part.

**Unblocks:**

- [Monotone elaboration](../gradual-typing/monotone-elaboration.md) — a
  resumption under a `Shares` frame's binding lands in a build cell the frame
  borrows.
- [A tenant's own scratch](../../cellgraph/roadmap/tenant-scratch.md) — the
  scratch that lets a tenant's lend drop at its release.
