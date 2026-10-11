# A merge's reach check

A merge whose cost does not grow with how deep its destination sits.

**Problem.** A merge — a pinned result or view, or a pinned birth into a half
that has already laid a branch — enters the home's branch into the
destination's, then follows the home's links branch by branch until it reaches
one the destination already reaches
([storage](../../lattice/src/types/identity.md#storage-a-root-and-its-branches)).
That check, [`TypeChain::reaches_branch`](../../lattice/src/types/chain.rs),
walks the destination's whole link chain on every branch the merge visits, so
a miss costs a pointer step per level above the destination. A tree cell d
levels deep that tails `Shares` merges its own branch into its host's: the
first check, against the predecessor's branch, misses after d steps, and the
next, against the branch the predecessor links, hits. So the hop costs O(d),
a recursion whose every level tails `Shares` costs O(d²), and one merge costs
at worst the product of the two chains' lengths. Node counts stay flat
(`each_way_a_deep_cell_hands_a_value_on_costs_the_same_however_deep` in
[the branch tests](../../src/scheduler/tests/branches.rs)), and no counter of
the walk reaches koan — the chain's `lookups` counter is `cfg(test)` inside
the lattice — so no test sees this cost.

**Acceptance criteria.**

- A merge's cost is the branches it enters plus a bounded number of steps per
  branch it visits, whatever the destination's link depth: a lattice test that
  merges one source into destinations linked 4 and 64 deep counts the same
  number of reach steps.
- A tree cell's `Shares` tail costs the same number of reach steps at depth 4
  and depth 16, pinned by a koan-side test beside the branch tests.
- Every node count the branch tests pin is unchanged, and every law in
  [tests/properties.rs](../../lattice/src/types/tests/properties.rs) holds
  unchanged.

**Directions.**

- *How a merge knows where to stop — open.* Options:
  - a **reach set**: gather the destination's reached branches in one pass up
    its chain, so each check is a set lookup and a merge costs the source chain
    plus the destination chain — still O(d) per merge, the common one-step case
    included;
  - a **fast path**: stop at once when the source branch is the destination's
    own or the one it links, which makes the spawned-child result O(1) but
    leaves the general bound;
  - **depth stamps**: each branch records its link depth, one more than its
    link's, to bound the walk by comparison — which needs an argument that a
    home on the destination's chain is found by depth, since a link skips the
    levels that laid no branch;
  - **no merge at a birth**: a `Shares` successor, which writes its host's
    branch, holds the pinned predecessor's branch as a link of its own — a
    second slot or a small fixed list on the tenant's side rather than on the
    host's branch;
  - **walk the source alone**: show that every pinned home lies on the
    destination's chain or is a descendant pinned into it, so the merge stops
    at the first source branch that links the destination, found without
    walking the destination's chain.

## Dependencies

[Shape branches](shape-branches.md) merge a shape branch at a tie through the
same door, so they pay the same check.

**Requires:** none — it reworks the merge door the branch tier ships with.

**Unblocks:** none — a leaf.
