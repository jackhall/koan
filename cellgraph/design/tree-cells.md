# Tree cells

The live slab is bounded and `create` refuses at the cap
([liveness-matrix.md § Bounding the two tiers](liveness-matrix.md#bounding-the-two-tiers)).
A non-tail recursion holds one pending cell per level, each awaiting its child,
so none of the admission answers that section lists applies: the pending
parents *are* the population, they cannot drain, and at a depth equal to the
cap admission deadlocks instead of degrading.

A call subtree needs none of what the matrix provides. Two facts make its
liveness a stack discipline. A parent outlives its children: a body waits on
its leading statements before its tail runs, and a reinstall retires an
incarnation only after they resolve. And a cell's values leave it only through
its terminal, whose destinations all lie on one ancestor chain. So before the
root's terminal nothing outside the subtree can pin a cell inside it, and
inside it every hold points up the chain. A hold that always points at a live
ancestor needs no bit and no count.

So a third region habitat beside the slab cell and the sealed cell: the **tree
cell**.

## Three habitats

| | slab cell | sealed cell | tree cell |
|---|---|---|---|
| identity | `Handle` (slot + generation) | `SealedId` | `TreeHandle` (pool index + generation) |
| liveness | pin column, birth column | holder count | structural: a parent outlives its children |
| holds it takes | its own pin row + sealed set | none (frozen aggregate) | none — mints go to its **root's** row and set |
| named by a mask | slab bit | id | **never** |
| can be entered | yes | no | yes |
| storage at death | reclaim / absorb / seal | retire | reclaim, or splice into an ancestor |

A tree cell lives **under a root** — a slab cell — through a chain of tree
parents, fixed at creation. Its **chain** is itself, its parents in order, and
its root. The pool it lives in takes no cap: the slab's cap is what bounds the
matrix, and a tree cell is in no matrix. What bounds the pool is the depth of
the call tree the embedder is running, which is the program's business.

What a tree cell records is the little that death needs: its root, its tree
parent, its depth, the count of undisposed children, the pledge below, and,
once it has died, where its bytes went. There is no resident table, no
continuation reach and no hold set.

## Mints, reach, and why no table is needed

Every placement into a tree cell mints its pinned reach into the **root's** pin
row and sealed-hold set, through the ordinary mint. A placement therefore
carries two names rather than one: the slab slot it mints into, and the cell
whose region takes the bytes.

A carrier homed in a tree cell reaches `{root}` and nothing else. That is
sound, not an approximation: its true reach is a subset of the root, the root's
pin row, and the root's sealed holds, by the mint invariant — and every place
the carrier can be pinned either mints into the root, where the and-not erases
the root's own bit and the rest is already there, or copies, which mints
nothing. The precise per-value mask is never read for a mint, and never read
for a price: a pin into a cell under the home prices at zero by the covered
shortcut, and a pin into an ancestor prices at the splice price, which is a
byte total rather than a walk. So nothing needs to remember it. A `keep` of a
tree-homed carrier interns no entry.

The seal transition needs no tree case. Tree cells hold nothing, so they are
never in a column; the root is, and its table is rewritten by the ordinary
transition.

## The ancestry rule

Applied to every operand homed in a tree cell `H`, at every placement door,
before the verdict — by where the destination `D` sits relative to `H`:

| `D` relative to `H` | what happens | why it is sound |
|---|---|---|
| `H` itself, or a tree cell **under** `H` | the ordinary door: the verdict is consulted, at a pin price of zero | `D` dies before `H` |
| an **ancestor** of `H` on its chain, or its root | the verdict is consulted with the **splice price**; on `Pin`, `H` and every intermediate are **pledged** to splice into `D` at death, the shallowest destination winning | `H`'s bump — and every intermediate's — will land in `D` or shallower, so an embedded borrow stays valid as long as `D` |
| anything else — a cousin under the same root, a cell under another root, an unrelated slab cell | a **forced copy**: the verdict is not consulted, and the build receives a severed view | nothing off `H`'s chain may outlive `H` while borrowing it, so a copy through the writer is the only thing that typechecks |

The classification compares depths and then walks parent links from the deeper
side for exactly the level distance, so it costs the distance between the two
and never the depth of the tree; two cells under different roots settle in
O(1).

This is koan's crossing rule — producer-born parts copy, references upward
cross free — made structural: an operand homed in the producer's own tree cell
is producer-born. The rule is named for what it turns on: every row of the
table above is an ancestry question, and nothing else is asked.

Operands homed in slab cells or in sealed cells take the ordinary path
whichever kind the destination is, with the mint redirected to the root.

## Death

`release_tree` takes **no absorption argument**. Where the bytes go was settled
at the placement door that pinned the terminal upward, and the pledge that door
left is what disposal reads.

A released cell is `Dead` at once: stale to every door, not a destination, not
a parent, not enterable, not releasable again. Its region stays put, because a
live child may still borrow it, and its pledge stays writable, because a
descendant's later upward pin may still walk through it.

If it has undisposed children it stops there — **dead-resident**, exactly as a
slab cell whose birth column is still named. That is what lets an embedder tear
a subtree down in any order: a parent failed by one branch's error is released
at once, and the sibling branches cascade into it as they die.

Otherwise it **disposes**, and the disposal walks up: each parent's child count
falls, a parent that is dead and childless disposes too, and at the top the
root's tree-child count falls and the slab's own disposal walk runs. The two
cascades are one walk.

Disposing one cell is O(1):

- **No pledge** — reclaim: the region drops, bundle and all.
- **A pledge** — splice: the whole bump moves into the destination's bundle. A
  `Bump` moves without moving a chunk byte. The destination is an ancestor and
  ancestors dispose after descendants, so it is always still there.

No hold arithmetic and no seal transition runs. The one count that moves is the
parent's, which is the birth tally's analogue rather than a hold count.

### The splice price

What the verdict is shown as `pin_bytes` for an upward pin: the bundle bytes of
`H` and of every intermediate up to the destination whose pledge is not already
that shallow or shallower. Marginal across the operands of one placement for
free — the pledge is applied the moment a verdict comes back `Pin`, before the
next operand is priced — so a second operand from the same home is shown
nothing.

Every intermediate is carried because a grandchild's arguments live in its
parent's storage: if the grandchild pledged straight to its grandparent and the
parent reclaimed, the grandparent's bundle would be left borrowing bytes that
are gone. The verdict is shown the whole cost, so an embedder that would rather
copy can.

The splice retains the dying cell's whole bump, garbage included; the copy the
verdict's other answer produces compacts and costs the value's size. Which one
happens is a pricing decision at the placement door, not a release-time option.

## Tombstones

A resident can be redeemed after its home died and spliced. Where the bytes are
now is answered by a **tombstone**: a dead cell whose identity any resident
could still name stays in the pool, pointing at the cell its bytes went to.

Tombstones are **never repointed** when their target's bytes move on in turn —
the chain lengthens instead. So a splice costs O(1) list work however many
tombstones hang off the dying cell, and a deep recursion that forwards each
terminal one level costs O(depth) in all rather than O(depth²). A redeem
follows the chain, one array load per hop: one hop per splice the bytes have
been through since the keep, which is one in the delivery shapes koan produces.

A chain ending at a slab handle continues through the slab's relocation map, so
a tree value spliced into a root that later reclaims, absorbs into its holder,
or seals needs no path of its own. The tombstone lists of a cell that leaves
the slab travel beside the relocation map, keyed by the handle they still name,
and are freed exactly when that entry is.

A cell nothing was ever kept in leaves no tombstone: no key can name it, so
nothing will ever ask. A cell that reclaims leaves none either — the bytes are
gone, a stale generation answers `Gone`, and the tombstones that had spliced
into its bundle are freed with it.

## Redeem

Entitlement inside a tree is **root identity**, which is O(1). Let `E` be the
executing cell, and read "root of `E`" as `E` itself when `E` is a slab cell.

| where the key's home resolves | entitled when | reach handed back |
|---|---|---|
| a live or dead-resident tree cell `T` | `root(T)` is `E`'s root | `{root(T)}`, homed in `T` |
| a live slab slot `S` | `S` is `E`'s root, or `E`'s root's pin row or birth row names it | the mask stored in that cell's resident table |
| a sealed cell `id` | `E`'s root holds `id` | `{id}` |
| nowhere — a recycled slot, or a chain that ends in a reclaim | — | `Gone` |

Same-root entitlement is sound because a redeem yields a **read**, which is
step-bounded and nothing dies inside a step, and a **carrier**, whose every
embedding goes back through the ancestry rule above — which is where ancestry
is actually checked.

A key that started in a tree cell indexes no table, whichever kind its chain
ends at: its value reached the root alone.

## Roots

A slab slot counts the tree cells whose chain tops out at it. It is not
disposable while that count is above zero, so a released root with a subtree
under it waits dead-resident exactly as one with a live slab descendant does,
and the last tree child's disposal is what sets its cascade off. Sealing or
absorbing a root moves its whole bundle, spliced tree bumps included — already
what a region does.

## What this retires

Group sealing. A subtree's internal holds never exist, so there is no chain of
sealed cells to collapse into one.

## Kind selection is the embedder's

The substrate ships both kinds and no rule for choosing between them. Which
cell a creation takes is an admission decision, made from the source edge's
destination, and it belongs to the layer that knows the destination. A producer
that delivers mid-life to an outside consumer — a yielding iterator — is a slab
cell, because an outside consumer can pin it while it lives.

## Open work

- [Rebuilding workgraph over cellgraph](../../workgraph/roadmap/adopt-cellgraph.md)
  — the first embedder's kind rule, its delivery walk's adoption of a tree
  terminal, and the scheduler-shaped tests over both kinds.
