# Liveness matrix

Liveness bookkeeping for the cell graph of [cellgraph.md](cellgraph.md): an
attributed bit matrix over a bounded live slab, plus an atomic sealed tier for
dead-but-still-reached regions. Nothing is reference-counted: a cell lives
while some bit names it, and is reclaimed the moment none does.

## The model

Live cells occupy a bounded pool — the **live slab** — owned by the cell
graph. A cell's identity outside the pool is a **handle** — pool slot index
plus generation — never an owning pointer; the pool is the sole owner of cell
slots, which is what makes reset-in-place recycling trivial rather than gated.
Cell *storage* is pointer-stable (chunked regions), so ownership of a
region's storage can leave the slab without a byte moving — sealing depends on
this.

Liveness over the slab is a bit matrix over pool slots. Bit (M, N) means
*cell M keeps cell N alive*, so a cell's **row** is its own hold set and a
cell's **column** is the set of cells holding it. The diagonal bit (M, M)
means *something is currently executing in cell M* — `enter` sets it for the
duration of a step instead of holding a clone. Cell N's liveness is its
column: **a cell whose column goes to zero is reclaimed on the spot.** There
is no other gate — no count arithmetic, no census of structural self-holds (a
cell's internal references to its own storage have no bits), no drop-ordering
discipline in the embedder's loop.

A cell that dies with a *nonzero* pin column does not linger in the slab: it
**seals** into the second tier, and its slot recycles. The one hold that keeps
a dead cell in place is a birth hold — the relation with no sealed form to
convert into, below — and it keeps it only until the last descendant naming it
dies in turn. The slab therefore holds live cells and the dead ones a
descendant's birth row still names, the embedder's admission control bounds
the live population, and the matrix is square at the slab cap — it never grows
with program retention. Slot reuse means slot index does not encode creation
order, so any ordering argument over the hold graph is a dynamic invariant
over the generations of live occupants, not a property of the matrix's shape.

**The hold graph must be acyclic for anything to reclaim, and the substrate
does not enforce that.** A ring — A holds B, B holds A — keeps both columns
nonzero forever, so a ring leaks rather than dangles: the failure is safe.
A locality merge below dissolves a ring it happens to meet — the source's hold
on its own target becomes a self-hold, which has no representation, and a
target left with no holder is reclaimed on the spot — so a ring whose members
die one at a time with nothing outside holding them is freed rather than
retained. That is a coincidence of the merges' triggers, not a collection
strategy: a ring an outside hold keeps above those triggers survives intact.
Preventing rings is the embedder's crossing discipline (koan's is the
anti-ring crossing rule of
[destination-homed-construction.md](../../design/destination-homed-construction.md));
the substrate ships no mint-time reachability check and no detector. What it
ships is `is_empty()`, the end-of-program alarm: a graph that is not empty
after the last release either forgot a release or carries a ring. Naming the
nodes on one is a walk of the hold graph, and the crate's own tests carry it
— a diagnostic for the substrate's tests, not a door on the substrate.

## The sealed tier

A sealed region is dead — nothing will ever execute in it, nothing will ever
be minted into it — and it survives only because other regions still reach
its storage. Only death with a nonzero column seals, and sealing is permanent: a
live-but-not-executing cell (one created and never entered, or entered and
awaiting re-entry) stays in the slab.

The tier is **atomic**: per-value reach tracking stops at its boundary. A
sealed region's reach is a single aggregate — its pin row, *frozen* at
death instead of cleared. Monotone holds make that row exactly the union
of every mask ever minted into the region, so the seal consults no storage
and scans no values: the aggregate is a word copy out of the matrix. The
region's storage chunks detach from the slot unmoved, the slot recycles under
a fresh generation, and the sealed region takes a **sealed id** — never reused,
so the tier needs no generations.

The id is two halves of a word: a graph-wide monotone **serial** above, and
below it the **index** of the sealed cell's slot in the tier's own dense slab.
The tier is that slab plus a free list of the indices retirement handed back,
sized at construction to the graph's cap, so a lookup is a bounds-checked load
and a serial compare rather than a hash. An index is reused; an id is not — the
serial beside a present sealed cell is what makes a retired id read as absent
rather than as whatever sealed cell later took its place. The serial leads the
word, so ordering ids is still ordering them by creation.

Three consequences:

- **A pin out of a sealed region is unrepresentable.** The tier has no
  rows; the frozen aggregate is the only outgoing edge set, and it only
  shrinks. Sealedness is enforced by what the structure cannot express, not
  by audit.
- **Sealed liveness is a holder count.** Reclamation of a sealed region is
  its count reaching zero. The count is decremented only in batch, at a
  holder's own death or reclamation, from the holder's hold set — the
  matrix's no-per-reason-release discipline, kept.
- **The hold graph is unchanged.** Freezing a row adds no edges: the graph
  over live ∪ sealed is the same graph, so retirement cascades still
  terminate wherever it was acyclic.

The aggregate must be a *hold*, not advisory metadata: if it were not, a slab
cell named only by frozen aggregates could hit column-zero and recycle, and a
later copy-out would mint a dangling bit. As a hold, the accounting is
uniform across tiers.

## Two relations, two structures

Holds arise from two sources with different disciplines, and each gets its
own structure rather than sharing one matrix:

- **Pin holds** — value reach. Cell M's region stores values whose borrows
  reach other regions' storage; each such region gets an entry in M's hold
  set. These accumulate throughout M's life.
- **Birth holds** — the parent chain. A cell names at most one **parent** at
  creation, and its birth row is derived by the substrate as the parent's
  birth row plus the parent's own bit — the cactus chain as masks, with
  transitive closure by construction rather than by embedder discipline. The
  row is written once at birth and never changes. Koan's use of the relation
  is the lexical outer chain a frame walks for variable lookup; the substrate
  knows only parents.

Keeping them separate lets each structure assert its own discipline: birth
bits are written once and are immutable; pin bits are monotone-growing. The
birth side is both a matrix and the sparser shape at once: the row answers
"is this cell an ancestor" in O(1), which is what `redeem` asks, and each slot
additionally records its **parent slot**, which is the axis a disposal walks.
The invariant that a birth row contains its parent's row ties the two
together. A cell leaves the slab when it is dead and no birth hold names
it — and only the pin row freezes into a seal: birth holds exist for
execution, so a cell's own birth row releases at its death unconditionally,
and a dead cell no descendant names has no birth presence left to convert.
What it does on the way out is what its pin column and its naming set decide:
reclamation when nothing reaches its storage, one of the two merges below when
exactly one thing does, and a seal otherwise. Storage that reaches
the parent chain does so through pin bits (transitive coverage puts those
regions in the row directly).

A release clears the dying cell's own birth row, which is the only write that
can bring another slot's birth-holder count to zero — birth bits are written
at creation and released wholesale at death, and no disposal touches them. So
the slots one release can free are exactly the released cell and the dead
ancestors above it, and by row containment they are a *prefix* of the parent
chain: a live ancestor, or one another branch still names, stops the walk and
everything above it is still held. The cascade is therefore a walk up the
parent links from the released cell, innermost first, with nothing scanning
the slab and no list of dead slots kept anywhere. Order does change retention
— a dead child pinning a dead parent that a live cell also pins reclaims
first and lets the parent absorb into the live cell — but the mirror image
favours the other order, so no fixed order dominates and the walk's own is
taken.

## Reach as a hybrid mask

A value's reach — the set of foreign regions its borrows keep alive — is a
bitmask over slab slots plus a sparse set of sealed ids:

- **Union and dedup are `OR`.** Composing reach for nested closures and
  collections is a bitwise OR of slab words and an idempotent union of sealed
  sets; deduplication is free.
- **Mint is a retention, literally.** Storing a value into cell M's region
  performs `row[M] |= mask` for the slab part and folds the sealed part
  into M's sealed-hold set. The destination's hold set *is* the retained
  reach.
- **The self rule is an AND-NOT.** A region must not hold itself alive
  (bit (M, M) is the executing flag, and a self-hold would be an
  unreclaimable cycle), so the mint masks off the destination's own bit:
  `row[M] |= mask & !bit(M)`.
- **Eternal storage contributes nothing.** Storage that outlives every cell
  owns no slot and no sealed id.
- **Reach is never folded.** Antichain minimization pays for itself only
  when each member costs an owning pointer; bits and ids cost nothing, so
  there is no subsumption step and no per-member pin semantics hook.
- **Both halves are inline at the width that matters.** The slab half always;
  the sparse half up to two ids, spilling to the heap only past that. So a
  reach naming at most two sealed regions is built, copied, rewritten by the
  seal transition, and compared without touching the allocator — and the merges
  keep a reach naming more than two sealed cells rare, since a chain of
  single-consumer producers collapses to the sealed cell at its head.

## The seal transition

When cell N dies with a nonzero column, three bounded maintenance steps convert
every representation of "N" from slab bit to sealed id `S_N`, and a fourth
folds in what the new sealed cell turns out to hold alone:

1. **Holders convert.** Column N names the live cells holding N. Each clears bit
   N from its own row, adds `S_N` to its sealed-hold set, and rewrites every
   mask in its **reach table** that names slot N (slab bit → `S_N`). That reach
   table is the only durable habitat a mask has on the slab side — the stored
   continuation's among them — so it is the only collection this step touches.
   Column N bounds who is scanned and the holder's reach table bounds the scan;
   nothing here is proportional to what a region stores. What keeps that bound
   from growing with a run is that the reach table holds one entry per
   *distinct* reach rather than one per value put to rest: entries are interned
   on content, so a cell kept into every step settles at the handful of shapes
   its keeps take, and a loop cart accumulated into for a thousand hops has one
   entry, not a thousand. Slab bits in live-region masks are therefore never
   stale — the rewrite is eager, and the mint OR stays untouched by any check.
2. **Frozen aggregates convert.** Each sealed region whose aggregate names
   slot N transfers bit N → `S_N`, and its contribution moves from column N
   to `S_N`'s count. The **reverse naming index** — per slab slot, the sparse
   set of sealed regions whose aggregate names it — locates them: a region
   registers under each slab bit of its aggregate when it seals, and
   unregisters at reclamation. One word rewritten per namer, never a storage
   scan.
3. **Storage detaches; the slot recycles** under a fresh generation.
4. **Count-1 holds absorb.** Every sealed region in the new sealed cell's
   aggregate whose holder count is 1 is held by this sealed cell and nothing
   else, so it folds in — seal-time absorption below. The candidate set is a
   worklist rather than one pass: a fold transfers ids the sealed cell did not
   name before, and drops a duplicated hold's count, either of which can newly
   qualify a region. The step runs after every seal, including a
   fold-into-namer, because a count can have dropped to 1 at any point since
   the holder's own seal.

The per-value masks *inside* N's own storage are not rewritten — they become
dead bytes. Nothing may read them, and the sealed tier's accessor makes that
structural: what a read out of a sealed region hands back is the value alone,
re-anchored at the reading borrow. The reach an at-rest value travels with is
the substrate's own bookkeeping and stays in the reach table, so the dead
per-value mask has no path out. Where a reach for such a value is *wanted* —
because a value kept in N is redeemed after N sealed — it is derived rather than
read, and it is `{S_N}` alone. A hold on `S_N` keeps its whole aggregate alive
transitively, so the id covers everything the value can read; it adds no direct
slab edge to the redeeming cell's row, so what the sealed cell reaches still
merges into it; and a mask naming only an id has no slab bit that could go stale
when the value is put to rest again under the redeeming cell. The derivation
belongs at the door that consumes it, not at the read. **The accessor is
reachable only inside an `enter` scope**: the read is a step transient of the
executing cell, so the value it hands out is covered by that cell's diagonal bit
until the step mints it somewhere or drops it.

Both access paths stay clear of sealed bytes. A live-slab list whose elements
borrow into sealed A composes its *own* maintained mask on shallow copy — the
copy's mask carries `S_A`, the destination holds A, and the copied spine's
borrows into A's unmoved storage stay valid. An element extracted out of A is
covered by `{S_A}`, whatever the dead per-value mask beside it in the storage
says.

## Invariants

The model is sound on a chain of invariants that must hold together:

1. **Monotone holds.** A hold set only grows during its owner's life, and
   freezes at seal. The sole releases are wholesale: the row clear when a
   live cell reclaims at column-zero, and the aggregate release when a sealed
   region's count reaches zero. There is no mid-life, per-reason release,
   which is what makes bit-setting idempotence safe: overlapping reasons for
   the same entry can never desynchronize, because nothing clears a single
   entry.
2. **Reach validity.** Every bit and id of a *readable* reach is covered by
   some live row or frozen aggregate; a value's reach is always covered by
   its host region's hold set (the mint OR establishes this); and a value
   only moves between regions while its current host is live — sealed hosts
   release values only through the accessor, which hands out no reach at all
   and leaves the aggregate covering what comes out.
3. **Retirement cascade.** A live cell reclaiming at column-zero clears its
   row and releases its sealed-hold set; the cleared entries name exactly the
   columns and counts worth re-checking. A sealed region reclaiming at
   count-zero releases its aggregate the same way. Acyclicity of the hold
   graph terminates every cascade.
4. **Fresh generations, monotone ids.** A reclaimed slot's next occupant
   carries a new generation, so stale handles are detectable; sealed ids are
   never reused, so a sealed reference cannot be re-bound at all.

## Why masks cannot go stale

Slab slots are reused, so a mask stored beside a value looks like it could
dangle. The argument closes tier by tier, with no per-bit version stamp:

- **Slab bits in live regions** are rewritten eagerly at the seal transition
  (column N names every live holder; invariant 2's covering clause puts every
  readable bit-N mask inside a column-N cell), and a slot recycles only after
  its seal or reclamation completes — so a readable slab bit always names the
  live occupant.
- **Sealed ids never dangle** — aggregates and sealed-hold sets are holds, a
  held count cannot reach zero, and ids are never reused.
- **Sealed storage is excluded from the argument** — its masks are dead bytes
  the accessor cannot return.

What the argument demands in exchange: **every habitat a readable mask can
live in must be covered by a hold.** There are exactly three habitats.

- **Values at rest in a region** — covered by the host's hold set via the mint
  OR. By construction. A cell created and never entered is a live host like
  any other: its continuation's captures rest in its region under its row.
  The mask itself lives in the host cell's reach table, never beside the
  value: that is what puts it where the seal transition can rewrite it, and
  what makes the at-rest carrier handed to an embedder not a habitat at all —
  it names a reach-table entry by a private key and carries no mask of its own.
  Two values that reach the same thing name one entry, which is safe because an
  entry is immutable content: no door writes one by index, and the only
  rewrites are the uniform ones the seal transition and a merge apply to every
  entry alike, carrying equal masks to equal masks.
- **Step transients** — covered by the executing cell's diagonal bit for the
  duration of the step. By construction. Values read out of a sealed region
  are step transients.
- **Eternal storage** — masks there are empty: escape severs reach,
  consistent with eternal storage owning no bit.

There is no in-flight habitat. A value crossing from one cell to another does
so inside a step of a live cell, either by being minted into the consumer
while the producer executes or by the consumer holding the producer as a cell
and reading it sealed later ([cellgraph.md § Passing values between
cells](cellgraph.md#passing-values-between-cells)). A free-standing envelope
that owns pins of its own is not a thing the substrate can express.

## Pool geometry

The slab width is a constant of the graph's *type*: a row of bits is `W`
words held inline and names `64 · W` slots, one word — 64 cells — by default,
and an embedder that wants a deeper slab instantiates a wider graph. So every
slab mask in a graph is that one width and the width question disappears; a
row is `Copy`, and building, copying, or comparing one touches no allocator.
Both relations are inline arrays of those rows, `64 · W × W` words each —
quadratic in the width by construction — held in the graph's own bytes, so a
graph wide enough for that to matter is one the embedder boxes.

The cap is a construction value at or below the width: `CellGraph::new`
refuses a cap above it, and admission is still refused at the cap, so a
two-cell graph over a 64-cell row is full at two. Growth within the cap
appends slots, and a bit for a slot that has not been handed out reads zero —
"no reach", which is always sound. The slab never compacts: renumbering live
slots would rewrite every stored mask outside the seal transition's bounded
scan. The sealed tier grows in its own id space and needs no geometry —
sealed sets are sparse.

## Bounding the two tiers

Occupancy decomposes into **live cells** (slab) and **retained
regions** (sealed tier), and the two populations are bounded by different
means.

The slab is bounded at its cap: `create` refuses when the slab is full, and
what to do then — admit lazily from an embedder-supplied generator of cell
creations, drain before creating more, fail the program — is admission
policy, which lives in the layer above. The dependency direction survives:
the substrate names no embedder type and holds no queue. Retention does not
occupy slab slots, so admission alone genuinely bounds this tier.

A call subtree does not occupy them either. Its cells live in the uncapped
tree pool ([tree-cells.md](tree-cells.md)), whose liveness is a stack
discipline rather than a matrix reading, so a non-tail recursion holds one
cell per level without ever consulting the cap — the depth of the call tree
the embedder is running is what bounds that pool, and nothing in the matrix
names a cell in it.

The sealed tier is program-dependent — a program building a deep cactus of
closures retains regions no matter how slowly cells are admitted — and it is
the *only* home of retention. Atomicity prices that home: a sealed region
retains everything it ever held, transitively through other aggregates, for
as long as anything holds it. The over-retention is deliberate — it is what
buys the O(1) seal — and it is relieved rather than prevented:

- **The consolidation lever.** Deep-copying a value out of a sealed region —
  the same transitive copy eternal escape and an explicit capture-severing
  `CLOSE OVER` perform in koan
  ([lazy-closures.md § Lazy close](../../design/lazy-closures.md)) — severs
  its reach and re-derives a precise mask. The aggregate prices the operation
  up front: an empty aggregate means nothing to do, and each entry names a
  region the copy would free the claim on.
- **Cell discipline bounds the common case.** A short-lived cell freezes a
  small row; the pathology is a long-lived cell that held much and then
  sealed — measurable as aggregate-attributable occupancy.
- **Pre-seal narrowing is rejected.** Recomputing a tighter aggregate from
  actually-reachable values at seal time is exactly the storage scan
  atomicity deletes.

Pricing the copy-versus-pin choice gains a sealed term, and the graph answers
it. The cost of holding sealed region S is the storage of its aggregate's
*transitive closure*: a walk of the hold graph from S that sums the chunk bytes
of every region it reaches, S's own included, billing a region two branches
both reach once. The walk spans **both** tiers — a live cell a reached
aggregate names is retention in waiting, since it will seal, or fold into its
namer, when it dies — so a closure is *frozen* exactly when it names no live
cell, and its price cannot change again. Nothing inside a frozen closure moves:
every node in it is named by a predecessor inside it, so none retires; a fold's
target is either the sealed cell a seal just minted or a namer whose aggregate
names a dying slab bit, and a frozen closure holds neither; and a sealed cell
whose sole holder lies inside the closure is never an absorption source either.
So a frozen closure's *sealed-cell set* is memoized on its sealed cell —
written into the sealed cell's own region, so `retained_bytes` counts the
memo's bytes like any other chunk, at chunk granularity — once, and exact for
the sealed cell's whole life, with no invalidation path to get wrong. It is the
set that is memoized and not a byte total: two branches of one closure may
share a sub-tier, so a walk that meets a memoized sealed cell merges its set
and the price sums once at the end. Summing memoized totals would bill the
shared part twice.

Double-billing *across* candidate decisions is the other half, and it is why
one query answers both cases. `unique_closures(candidates)` prices each
candidate at the slice of its closure no *other* candidate reaches, so a
sub-tier two releases share is billed to neither and each answer is the honest
marginal price of releasing that one hold; a lone candidate shares its closure
with nobody, so its slice is the whole of it and the single-sealed-cell price
needs no query of its own. Uniqueness is relative to the candidate set: a
holder from outside it is not discounted, which is why a candidate lying inside
another candidate's closure prices at zero. Discounting outside holders is a
dominator computation over the hold graph, and is [unplanned
work](../roadmap/README.md#unplanned-work).

The other price is the one a placement turns on: what pinning **this** operand
into **this** destination would newly retain. It is marginal. The walk starts
from the operand's reach with the destination itself, everything the
destination's pin row names, and every sealed cell it already holds pre-marked
as seen, so storage the destination is answerable for anyway is billed to
nobody, and an operand homed in the destination — or in a cell it holds
directly — prices at zero. When every seed is already covered the walk is not
built at all: the answer is zero after one containment test, which is the
common case of placing a value into a cell that already holds its home.

It is marginal within the placement, too. Each operand is priced against what
the operands before it have already pinned, so a placement walks each distinct
source once and the prices *sum* to what the placement newly retains, rather
than billing a shared source once per operand. Operands are priced in the
embedder's list order — the first operand from a shared source is the one
shown the shared cost — since reach width is not price and the embedder
already controls the list.

The pruning is at *direct* holds and not at the destination's whole closure: a
node reached only through a directly held node is still billed. The figure therefore only ever over-bills, which biases the
answer toward copying and never toward a pin whose cost the embedder was not
shown.

The pressure model reads these prices beside the occupancy of both tiers at
that instant — slab slots occupied against the cap, sealed cells in the sealed
tier, the bytes those sealed cells retain (a running total rather than a scan),
and the destination region's own size. The substrate ships numbers and no
threshold: whether the copy-versus-hold ramp is linear on occupancy or a step
at a watermark is the embedder's
([adopt-cellgraph.md](../../workgraph/roadmap/adopt-cellgraph.md)).

**Every one of these queries is crate-private**, and so is the vocabulary they
speak — the mask, the sealed id, the closure and occupancy answers. A price is
worth something only at the moment a decision turns on it, and the id or mask
it is asked about is not a thing an embedder can hold: an embedder that could
name one could pair a reach with a value of its own choosing, which is the one
forgery the carrier types exist to prevent. So the substrate hands out no
price directly. It hands out one bundle of numbers, computed for the decision
that turns on it, at the **crossing verdict** — the embedder closure the
graph is constructed with and consults once per operand of every placement,
the destination-homed one and the capturing successor store alike. That
bundle is the one type in the whole surface that names a price, and the
answer it takes back is one of two words. A pin mints the operand's reach
into the destination and hands the build the borrow at the destination's own
region brand; a copy mints nothing and hands the build a view at an unrelated
brand, so embedding it is a compile error and the only copy that typechecks
is a deep one through the writer
([cellgraph.md § The crossing verdict](cellgraph.md#the-crossing-verdict)).
Every query is read-only: none changes a hold, and no path inside the
substrate consults one, so pricing is never a cost the seal transition
pays.

## Locality tactics

These tactics keep the degenerate cases — column-zero reclamation, empty
aggregates, flat sealed cells — common. All are heuristics: they exploit shapes
most programs exhibit most of the time, and none is a guarantee. One line
disciplines the absorptions among them: **an absorption is legal exactly when
it preserves its target's reach discipline.** A dying cell's per-value masks
are still maintained — seal step 1 rewrote them all its life — so it may merge
into a live region; sealed merges with sealed, where both sides derive reach
from aggregates. Only moving *already-sealed* storage back into the live tier
is barred: its per-value masks are dead bytes, and the severing copy remains
the lever there.

- **The crossing rule keeps cells out of the tier.** The general crossing
  tier of
  [destination-homed-construction.md](../../design/destination-homed-construction.md)
  — producer-born parts copy; references upward and sideways cross free —
  composes with the self rule: a caller-homed reference delivered back to
  its own region contributes the caller's own bit, which `mask & !bit(C)`
  erases at the mint. A per-call cell whose result crosses under the rule
  therefore delivers with no retained reach and reclaims at column-zero without
  sealing. This holds only where the rule is enforced and only for cells no
  reference escapes downward from — closures over locals and stored handles
  still seal, by design. Whether the rule applies at every adopt or only at
  yield deliveries is open per
  [yielding-iterators.md](../../roadmap/foundation/yielding-iterators.md).

- **Destination-homed construction keeps loop hops out of the tier.** A
  value built directly into another live cell's region — the step context's
  placement into a cell by handle — never exists in the producer's region,
  so a tail hop that builds the next hop's arguments into the next hop's cell
  leaves the retiring cell with a zero column: no copy, no pin, no seal. The
  operands are read as step transients under the retiring cell's diagonal
  bit.

- **Death-time absorption ties a dying cell to its unique live holder.** When N
  dies with `column(N) == bit(M)` and an empty naming set, N never seals: its
  chunks splice onto M's region, its frozen row ORs into `row[M]` through the
  standard `& !bit(M)` mint, and every mask in M's reach table naming slot N is
  rewritten to nothing — the cross-region hold becomes a structural self-hold,
  which has no bits. That rewrite is the same bounded scan seal step 1 performs,
  and the trigger means M is the only cell scanned. N's own dormant carriers
  survive the merge rather than dying with a sealed cell: their masks move into
  M's reach table, re-homed bit-for-bit and appended at a first index, and a key
  minted under N's handle is forwarded to that offset. A dormant carrier
  therefore survives any number of merges, and the forwarding costs one entry
  per departed cell however many values it kept. M is any *slab occupant*, live
  or dead-but-undisposed: "live holder" names the slab tier as opposed to the
  sealed one, and a dead cell a descendant's birth row still names keeps a
  maintained row, so absorbing into it only brings forward the fold its own
  disposal would perform. The and-not is also what dissolves a two-cell ring the
  merge meets: N's hold on its own holder M lands nowhere. No sealed id, no
  index entry, no accessor indirection: reads stay on the precise per-value-mask
  path, and future seal transitions maintain the absorbed masks automatically
  because the chunks are M's storage now. Retention is identical to sealing — a
  count-1 sealed region lives exactly until its holder's release anyway — so the
  sealed cell and the indirection are deleted without retaining more. Where it
  pays: a long-lived single consumer absorbs each kept per-iteration producer as
  it dies instead of confettiing the sealed tier with count-1 sealed cells; a
  loop's final cart is absorbed into the loop's consumer, which delivers the
  loop result without a copy even though which iteration is final is decided at
  runtime; and a stash-first result — the case destination-homed construction's
  refusal rule deliberately declines — transfers wholesale without the delivery
  copy. Two guards. Absorption is wholesale where the crossing-rule copy is
  precise — dead locals ride along, permanently indistinguishable inside M — so
  a cell that stored much and delivers little should copy instead; region size
  is known, result size is not, a priced choice. A loop does not pay that price
  at all, because a loop is three cells rather than two: two per-hop cells that
  alternate, and one longer-lived **cart** the results accumulate into. A hop
  builds the next hop's arguments into the next hop's cell and its own results
  into the cart, both by destination-homed placement, so the retiring hop dies
  at column zero and is *freed* — never absorbed, never sealed. The cart
  accretes only what is built into it: a replaced carried value leaves its dead
  predecessor in the cart's own bump, and nothing arrives from elsewhere. So the
  signal a consolidation copy is decided on is the cart's own size, which the
  crossing carries as the destination's chunk bytes ([§ Bounding the two
  tiers](#bounding-the-two-tiers)); there is no absorbed-bytes figure to keep,
  because no absorption feeds the cart. The price above is why this merge, alone
  of the three, is **refusable**: `release` carries the embedder's choice,
  recorded on the slot and read when the slot actually disposes — later than the
  release, for a cell a descendant's birth row still names. A refusal falls
  through to the plain seal. The two sealed-tier merges retain exactly what a
  plain seal retains, so there is nothing there to price and no refusal to
  offer.

- **Seal-time absorption flattens ownership chains.** At P's seal, a sealed
  region S in P's hold set with holder count 1 is held by P alone — and mask
  validity plus aggregates-being-holds make P's sealed cell the only place in
  the system that names S, so absorbing S into P is purely local: OR S's
  aggregate into P's, splice S's storage chunks onto P's (pointer-stable),
  repoint S's reverse-naming-index entries. The candidate test is bitwise and
  bounded by P's own hold set — `count(S) == 1` per sealed entry (the live
  tier's analogue of the trigger is death-time absorption above). The
  candidates are a worklist, not one pass: folding S in transfers the ids S
  held onto P, where a count of 1 qualifies them in turn, and a hold P and S
  both had duplicates rather than transfers, dropping that region's count by
  one. Where S's aggregate names P itself — S held its own holder — the fold
  drops the hold instead of recording it, and if that was P's last holder the
  sealed cell is reclaimed on the spot: a sealed ring dissolves through the
  merge that met it. When call trees follow a single-consumer shape, chains
  collapse into flat sealed cells — one count, one folded aggregate, one
  storage bundle — and releasing one is a depth-one cascade; a program whose
  retained regions are genuinely shared absorbs nothing and keeps the full
  graph. For absorbed chains the uniquely-held slice of the pricing model is
  read directly off the merged sealed cell; the shared residue still poses the
  cross-decision double-billing question above. The merge is per region, and it
  needs no group case: a dying call subtree is not a group of slab cells at all
  but a chain of [tree cells](tree-cells.md), whose internal holds never exist,
  so there is no chain of sealed cells to collapse into one and only the
  subtree's boundary reach ever reaches the sealed tier.

- **Fold-into-namer covers the downward direction.** When N dies with a zero
  column and a singleton naming set {Q}, the sealed Q is provably N's only
  namer — the same mask-validity-plus-aggregates-are-holds argument as above,
  pointed the other way — so N seals *into* Q rather than minting a sealed
  cell: N's chunks splice onto Q's, N's frozen row ORs into Q's aggregate
  (registering Q under any slab bits new to it), and the slots N's row named
  trade bit N for Q in their naming sets. The work is the seal transition's own
  steps minus the sealed-cell creation, and the test reads two structures the
  transition already holds: the column and the naming set. No stored mask needs
  rewriting either: a stored mask naming a slot implies a pin hold on it, and
  this cell's column is empty by the trigger. Where N's own row named Q — N
  held the sealed cell that names it — the fold drops that hold, and a Q left
  with no holder is reclaimed on the spot, which is how the last cell of a ring
  frees the rest of it. Together with seal-time absorption this covers both
  ends of an ownership chain; both remain sealed-tier-only merges, per the
  discipline line above.

## Failure direction

A reference count fails safe: a forgotten release leaks. This model fails
dangerous: a forgotten bit reclaims a live cell. That trade is accepted
deliberately, and it dictates the engineering posture: the matrix, the sealed
tier, and every hold transition are encapsulated behind a narrow interface
designed so that safe usage cannot skip a declaration — a value cannot be
stored without its mask passing through the mint OR, and a sealed region's
contents cannot be read except through the accessor, which hands back no mask
to re-pair — and the encapsulated core is tested exhaustively (property tests
over hold/seal/retire interleavings, plus the Miri slate) rather than audited
by convention. The surface is narrow in the literal sense too: what an embedder
can name is fixed by an integration test that names all of it and is run under
both build profiles, so neither a widening nor a profile-dependent door passes
unnoticed. The one failure that is *not* dangerous is the ring: a hold cycle
leaks unless a merge dissolves it, `is_empty()` reports that one survived, and
the ring walk in the crate's tests names the nodes on it.

## Layout

Row-major by holder: one contiguous run of words per slot, naming the set
that slot keeps alive. That is the axis each relation's *write* wants, which
is what decides the orientation. Both compound writes are whole-row ORs — the
birth derivation ORs a parent's row into its child's, the pin mint ORs a
reach mask into a destination's — the row clear at reclamation zeroes a run,
and the freeze at seal copies one out. Every one of them is a word-wise pass
over contiguous memory that reads no value. The reclaim query asks the other
axis — "does anything still hold this cell" is a column, and a column has no
run of its own — so each matrix carries a **holder tally**, one count per
column, bumped by every write that can set or clear a bit. The query is then
one read, and a write pays only for the bits it *newly* sets.

Per sealed region: one hybrid aggregate mask, one holder count, one head word
naming its lineage, and its storage, which is a *bundle* of regions, not one.
Every merge splices the absorbed chunks in whole rather than copying them, so a
region is the bump it writes into plus the bumps of everything folded into it;
the chunks keep their addresses, which is the same pointer stability a detached
seal already relies on. Nothing is ever allocated into an absorbed bump again,
so a long chain keeps each link's chunk headroom rather than compacting it. The
memoized closure lives in that storage too — a run of ids in the sealed cell's
own bump, written once and read back only through the region that wrote it — so
the sealed cell's bookkeeping is its own bytes and its price says so.

Per slab slot: one sparse reverse-naming set, one reach table — the reaches of
the values kept in that cell, interned on content and named by index, and the
only durable habitat a slab-side mask has — and one head word naming the lineage
of departed cells whose dormant carriers it absorbed. The reach table is bounded
by the *distinct* reaches the cell has been kept into, not by how many times, so
it does not grow with the length of a run; a merge is the one writer that
appends without interning, since the moved block's position is what forwards its
keys.

Per graph: one relocation entry per departed cell, saying where its dormant
carriers went — a live slot at a first index, or a sealed cell — held in one
list per slab slot and searched by the handle's generation rather than hashed. A
lineage is a chain threaded through those entries themselves, which is why a
slot and a sealed cell each carry a head word and no collection: the links live
where the entries already are. The lineages and the map are bounded by merges
rather than by values — a departing cell contributes one entry however many
values it kept, dropped when its target reclaims or retires — so a slot's list
holds two entries inline and reaches the allocator only past that. Nothing here
hashes, in either direction.

Per tier: the dense sealed-cell slab and its free list, both reserved at the
graph's cap so a seal grows neither, and one running retained-byte total,
maintained where storage enters or leaves the sealed tier rather than summed on
demand. The holder tallies above are derived data — derived *from* attributed
transitions, never a free-standing count a caller could release against — and
invisible to the interface: under test each tally is asserted against the
column scan it stands in for, at every query that reads it.

## Open work

One koan-side primitive the model leans on is tracked on koan's own roadmap:
[Yielding iterators](../../roadmap/foundation/yielding-iterators.md) —
producers that yield many values before dying, the surface family lazy
admission belongs to.
