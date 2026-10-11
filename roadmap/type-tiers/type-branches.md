# Type branches

A branch of the type lattice per region, so a type the run builds lives as
long as the values that carry it.

**Problem.** The type registry is one table in program storage, released
whole when the program ends
([storage](../../lattice/src/types/identity.md#storage-a-root-and-its-branches)).
Every type the run builds stays until then. Growth is per distinct content, not
per evaluation, so it bites where a program builds types over varying values:
an opaque view mints one carrier per distinct content
([the view door](../../src/knot/module/README.md#the-view-door)), then a view
signature at that carrier and every list, record and function type built over
it, and a functor capturing an effect-drawn value does so on every run of its
action; code built at the run has its shape typed into the registry per distinct
program; a nominal born per call is a new type each call
([limits](../../design/quantified-types.md#limits)). A loop that builds a module
per iteration therefore grows the registry without bound while every value it
built is released with its hop.

**Acceptance criteria.**

- A type the load builds lives in program storage for the life of the program;
  a type the run builds lives in the habitat of the value it is laid down with —
  a region, or a scratch habitat, whose branch is released at its reset; every
  handle a value carries resolves from that value's own habitat, in its branch,
  its region's branch or the root. A debug check beside the carried-type check
  reads it after every crossing door.
- A tail recursion that ascribes a module of new content each hop, placed
  `Fresh`, holds a constant number of interned nodes across hops, pinned by a
  test beside the flat-heap tail test.
- A copied operand re-interns each part's memo into the destination's branch
  inside the deep copy, and interns nothing for a part the seen type hides.
- A pinned operand's home branch is merged into the destination's branch at the
  pin, and no reader walks the pinned value for its types.
- A type value crossing re-interns the type it names, not only its memo.
- A door interns into the branch of the habitat its writer allocates into, and
  interning into another habitat's branch does not compile.
- A verdict is recorded in the shortest-lived tier holding one of its two
  types, so a verdict over a branch's type is released with the branch, and one
  over two root types is kept in the root's table.
- A branch's table is a lattice
  [`DigestTable`](../../lattice/src/bump/digest_table.rs), laid at its
  smallest size in its region.
- Every law in [tests/properties.rs](../../lattice/src/types/tests/properties.rs)
  holds unchanged over a chain of two tiers, and the lattice adds no `unsafe`.

**Directions.**

- *Two tiers with their own head arrays — decided.* The root holds the load's
  types; each region's branch holds the run's, parent the root, created on the
  first intern. A region's lookup is two probes, and a scratch habitat's
  branch adds a third, below. A chain through callers would break under
  tail hops, since cells are peers related by reach
  ([tail hops](../../src/scheduler/README.md#tail-hops)), and branch entries
  chained into the root's heads would need unlinking at region death, against
  O(1) drop-free teardown.
- *Where a type is interned — decided.* Into the branch of the habitat the
  value is laid down in, reached through the `Header` handed out beside the
  `Writer` that lays it. A `Shares` tenant interns
  into its host's branch as its bytes go there; a result built into a home
  interns into the home's; a value built in a scratch habitat interns into the
  habitat's branch, which resets with it.
- *How a door reaches a habitat's branch — decided.* The branch is the
  habitat's [header](../../cellgraph/README.md#the-contract-four-embedder-types): the
  substrate hands each habitat half's header out through a `Header` door beside
  the half's `Writer`, both invariant in the half's brand, so every door that
  lays a value has the branch of the habitat it lays into and none has
  another's. A crossing reads its operand's home branch through the operand's
  `HomeHeader`, inside a closure. The cell that owns a region lays its branch
  at its first step; a tenant or a producer
  building into a home only reads the slot, and finds a branch laid, since every
  region has run its owner's step before anything foreign writes into it. A
  scratch branch is laid by the first door that interns there, since a reset
  clears it.
- *A scratch branch reads through its region's — decided.* A scratch value and
  a region value meet in one relation, so while a step's scratch branch holds
  anything the step reads three tiers: the scratch branch, the region's, then
  the root. A build delivered into another cell's scratch habitat takes no
  operands, so it reads its own branch and the root.
- *Where a verdict is recorded — decided.* In the verdict table of the
  shortest-lived tier holding one of its two types, laid at the tier's first
  such verdict. A handle lives in exactly one tier of a chain, so the tier is
  well defined, and verdicts are never load-bearing, so the placement is a
  matter of retention alone: a run's verdicts over the types it builds no longer
  evict the load's from the root's fixed table. A lookup probes each tier's
  table that exists, shortest-lived first.
- *A pin merges the home's branch — decided.* Visibility is not bounded by a
  value's top memo: under `Any`, a union, a function type or a carrier a part
  keeps its own memo ([the door](../../src/values/README.md#the-type-memo-and-satisfies)),
  so only a walk of the value bounds what a reader sees, and a pin must not
  walk. The merge costs the types the home minted, never the value. The
  alternative, probing the branch of each cell the destination reaches, puts
  reach into the lattice and a probe per reached cell on every lookup.
- *Table structure — decided* per
  [storage](../../lattice/src/types/identity.md#storage-a-root-and-its-branches).
  A branch is a `DigestTable` laid in its region, which grows by laying fixed
  runs, since a region's `Writer` has no allocator. Koan implements the
  lattice's `LaysRuns` for a newtype over the region's `Writer`, since neither
  crate names the other.
- *Probe order — decided.* Root first, then the branch; an empty branch is
  skipped, which lazy creation gives. The lattice's
  [node-table benchmark](../../lattice/benches/node_table.rs) puts the root
  miss a branch handle pays over a hashbrown root at under half a nanosecond,
  nothing worth a hint from the elaborator about which tier a site's handles
  can live in; that hint is a later lever of
  [presizing a branch](branch-presizing.md).
- *Sizing a branch — decided.* A branch starts small and grows, as the root
  does. Growth by doubling strands one run per branch, two thirds of a
  256-entry branch's node bytes with 16 B slots, and dominates the branch's
  build; laying a branch at a predicted size is
  [presizing a branch](branch-presizing.md).

## Dependencies

A branch is also the scratch lattice a
[refused program](../rewrite/refused-programs-stay-loaded.md) could hold its types in.

**Requires:** none.

**Unblocks:**

- [Activation typing](../gradual-typing/activation-typing.md) — a pass per
  activation interns into the activation's branch.
- [Presizing a branch](branch-presizing.md) — a branch laid at a predicted
  size instead of grown from its smallest.
- [Shape branches](shape-branches.md) — a branch per sealed `Shares` body,
  laid beside the region tier.
