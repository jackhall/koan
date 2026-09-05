# Liveness matrix

Liveness bookkeeping for pool-resident frames: an attributed bit matrix over a
bounded live slab, plus an atomic sealed tier for dead-but-still-reached
regions. This design is aspirational: nothing in it is implemented, and it
supersedes the reference-counted holds that [reach.md](reach.md) and the
scheduler's anchor plumbing describe today.

## The model

Live frames occupy a bounded pool — the **live slab** — owned by the node
store. A frame's identity outside the pool is a **handle** — pool slot index
plus generation — never an owning pointer; the pool is the sole owner of frame
slots, which is what makes reset-in-place recycling trivial rather than gated.
Frame *storage* is pointer-stable (chunked regions), so ownership of a
region's storage can leave the slab without a byte moving — sealing depends on
this.

Liveness over the slab is a bit matrix over pool slots. Bit (N, M) means
*frame M keeps frame N alive*. The diagonal bit (N, N) means *something is
currently executing in frame N* — the drain marks it for the duration of a
step instead of holding a clone. In the row-major layout, frame N's liveness
is one contiguous row: **a frame whose row goes to zero is reclaimed on the
spot.** There is no other gate — no count arithmetic, no census of structural
self-holds (a frame's internal references to its own storage have no bits),
no drop-ordering discipline in the drain loop.

A frame that dies with a *nonzero* row does not linger in the slab: it
**seals** into the second tier, and its slot recycles. The slab therefore
holds live frames only, admission control bounds the live population, and the
matrix is square at the slab cap — it never grows with program retention.
Frames are well-ordered by creation order, which is what guarantees the hold
graph is acyclic; slot reuse means slot index does not encode that order, so
acyclicity is a dynamic invariant over the generations of live occupants, not
a property of the matrix's shape.

## The sealed tier

A sealed region is dead — nothing will ever execute in it, nothing will ever
be minted into it — and it survives only because other regions still reach
its storage. Only death with a nonzero row seals, and sealing is permanent: a
live-but-not-executing frame (a parked or suspended node) stays in the slab.

The tier is **atomic**: per-value reach tracking stops at its boundary. A
sealed region's reach is a single aggregate — its pin column, *frozen* at
death instead of cleared. Monotone holds make that column exactly the union
of every mask ever minted into the region, so the seal consults no storage
and scans no values: the aggregate is a word copy out of the matrix. The
region's storage chunks detach from the slot unmoved, the slot recycles under
a fresh generation, and the sealed region takes a **sealed id** from a
monotone space — never reused, so the tier needs no generations.

Three consequences:

- **A pin out of a sealed region is unrepresentable.** The tier has no
  columns; the frozen aggregate is the only outgoing edge set, and it only
  shrinks. Sealedness is enforced by what the structure cannot express, not
  by audit.
- **Sealed liveness is a holder count.** Reclamation of a sealed region is
  its count reaching zero. The count is decremented only in batch, at a
  holder's own death or reclamation, from the holder's hold set — the
  matrix's no-per-reason-release discipline, kept.
- **The hold graph is unchanged.** Freezing a column adds no edges: the graph
  over live ∪ sealed is the same graph that was already acyclic by creation
  order, so retirement cascades still terminate.

The aggregate must be a *hold*, not advisory metadata: if it were not, a slab
frame named only by frozen aggregates could hit row-zero and recycle, and a
later copy-out would mint a dangling bit. As a hold, the accounting is
uniform across tiers.

## Two relations, two structures

Holds arise from two sources with different disciplines, and each gets its
own structure rather than sharing one matrix:

- **Pin holds** — value reach. Frame M's region stores values whose borrows
  reach other regions' storage; each such region gets an entry in M's hold
  set. These accumulate throughout M's life.
- **Lexical holds** — chain topology. A frame keeps its lexical outer chain
  alive for execution — variable lookup along the chain. These are fixed at
  frame birth (a frame's lexical hold set is its outer's set plus the outer's
  own bit — the cactus chain as masks) and never change afterward.

Keeping them separate lets each structure assert its own discipline: lexical
bits are written once at birth and are immutable; pin bits are
monotone-growing. It also leaves the lexical side free to take a sparser
shape than a matrix. A frame is reclaimable when it is dead in *both*
relations — and only the pin column freezes into a seal: lexical holds exist
for execution, so they release at death unconditionally. Storage that reaches
the outer chain does so through pin bits (transitive coverage puts those
regions in the column directly).

## Reach as a hybrid mask

A value's reach — the set of foreign regions its borrows keep alive — is a
bitmask over slab slots plus a sparse set of sealed ids:

- **Union and dedup are `OR`.** Composing reach for nested closures and
  collections is a bitwise OR of slab words and an idempotent union of sealed
  sets; deduplication is free.
- **Mint is a retention, literally.** Storing a value into frame M's region
  performs `column[M] |= mask` for the slab part and folds the sealed part
  into M's sealed-hold set. The destination's hold set *is* the retained
  reach.
- **The self rule is an AND-NOT.** A region must not hold itself alive
  (bit (M, M) is the executing flag, and a self-hold would be an
  unreclaimable cycle), so the mint masks off the destination's own bit:
  `column[M] |= mask & !bit(M)`.
- **The eternal rule disappears.** Storage that outlives every region simply
  owns no slot and no sealed id, and contributes nothing.
- **Subsumption disappears.** Antichain minimization pays for itself only
  when each member costs an owning pointer; bits and ids cost nothing, so
  reach is never folded, and the per-member pin semantics hook (`PinsRegion`)
  and its unsafe contract go with it.

## The seal transition

When frame N dies with a nonzero row, three bounded maintenance steps convert
every representation of "N" from slab bit to sealed id `S_N`:

1. **Holders convert.** Row N names the live frames holding N. Each clears
   column bit N, adds `S_N` to its sealed-hold set, and rewrites its *stored*
   per-value masks that name slot N (slab bit → `S_N`). Row N bounds who is
   scanned; the slab cap bounds the whole rewrite. Slab bits in live-region
   masks are therefore never stale — the rewrite is eager, and the mint OR
   stays untouched by any check.
2. **Frozen aggregates convert.** Each sealed region whose aggregate names
   slot N transfers bit N → `S_N`, and its contribution moves from row N to
   `S_N`'s count. The **reverse naming index** — per slab slot, the sparse
   set of sealed regions whose aggregate names it — locates them: a region
   registers under each slab bit of its aggregate when it seals, and
   unregisters at reclamation. One word rewritten per namer, never a storage
   scan.
3. **Storage detaches; the slot recycles** under a fresh generation.

The per-value masks *inside* N's own storage are not rewritten — they become
dead bytes. Nothing may read them, and the sealed tier's accessor makes that
structural: a read out of a sealed region returns the value with reach
*derived* as `{S_N} ∪ aggregate(N)` — over-approximate, but covering, since a
resident value's true reach is a subset of the region's holds by mint-time
coverage. The per-value mask is unreachable through the interface, which is
what entitles the seal to skip the storage scan.

Both access paths stay clear of sealed bytes. A live-slab list whose elements
borrow into sealed A composes its *own* maintained mask on shallow copy — the
copy's mask carries `S_A`, the destination holds A, and the copied spine's
borrows into A's unmoved storage stay valid. Extracting an element out of A
derives `{S_A} ∪ aggregate(A)` at the accessor.

## Invariants

The model is sound on a chain of invariants that must hold together:

1. **Monotone holds.** A hold set only grows during its owner's life, and
   freezes at seal. The sole releases are wholesale: the column clear when a
   live frame reclaims at row-zero, and the aggregate release when a sealed
   region's count reaches zero. There is no mid-life, per-reason release,
   which is what makes bit-setting idempotence safe: overlapping reasons for
   the same entry can never desynchronize, because nothing clears a single
   entry.
2. **Mask validity.** Every bit and id of a *readable* mask is covered by
   some live column or frozen aggregate; a value's mask is always covered by
   its host region's hold set (the mint OR establishes this); and a value
   only moves between regions while its current host is live — sealed hosts
   release values only through the accessor, which re-derives reach.
3. **Retirement cascade.** A live frame reclaiming at row-zero clears its
   column and releases its sealed-hold set; the cleared entries name exactly
   the rows and counts worth re-checking. A sealed region reclaiming at
   count-zero releases its aggregate the same way. Acyclicity of the hold
   graph terminates every cascade.
4. **Fresh generations, monotone ids.** A reclaimed slot's next occupant
   carries a new generation, so stale handles are detectable; sealed ids are
   never reused, so a sealed reference cannot be re-bound at all.

## Why masks cannot go stale

Slab slots are reused, so a mask stored beside a value looks like it could
dangle. The argument closes tier by tier, with no per-bit version stamp:

- **Slab bits in live regions** are rewritten eagerly at the seal transition
  (row N names every live holder; invariant 2's covering clause puts every
  readable bit-N mask inside a row-N frame), and a slot recycles only after
  its seal or reclamation completes — so a readable slab bit always names the
  live occupant.
- **Sealed ids never dangle** — aggregates and sealed-hold sets are holds, a
  held count cannot reach zero, and ids are never reused.
- **Sealed storage is excluded from the argument** — its masks are dead bytes
  the accessor cannot return.

What the argument demands in exchange: **every habitat a readable mask can
live in must be covered by a hold.**

- **Region-resident values** — covered by the host's hold set via the mint
  OR. By construction.
- **Step transients** — covered by the executing frame's diagonal bit for the
  duration of the step. By construction.
- **In-flight deliveries** — a produced value awaiting delivery at finalize
  sits in neither habitat. Either the producer frame stays live until the
  delivery OR lands in the consumer's hold set, or the scheduler owns a
  pseudo-column for the in-flight band. Undecided.
- **Eternal storage** — masks there are empty: escape severs reach,
  consistent with eternal storage owning no bit.

## Pool geometry

The slab is fixed at its cap, so slab masks are fixed-width and the width
question disappears; growth within the cap appends slots, and an old mask
read at a wider width zero-extends — a zero bit means "no reach", which is
always sound. The slab never compacts: renumbering live slots would rewrite
every stored mask outside the seal transition's bounded scan. The sealed tier
grows in its own id space and needs no geometry — sealed sets are sparse.

## Bounding the two tiers

Occupancy decomposes into **executing frames** (slab) and **retained
regions** (sealed tier), and the two populations are bounded by different
means.

The slab is bounded by admission control: rather than the embedder eagerly
scheduling every node, it hands the scheduler an iterator of node creations,
and the scheduler consumes it against its current resources. The dependency
direction survives — workgraph names no embedder type, so a lazy,
embedder-supplied generator is a clean interface. Retention no longer
occupies slab slots, so admission alone genuinely bounds this tier.

The sealed tier is program-dependent — a program building a deep cactus of
closures retains regions no matter how slowly nodes are admitted — and it is
now the *only* home of retention. Atomicity prices that home: a sealed region
retains everything it ever held, transitively through other aggregates, for
as long as anything holds it. The over-retention is deliberate — it is what
buys the O(1) seal — and it is relieved rather than prevented:

- **The consolidation lever fits unchanged.** Deep-copying a value out of a
  sealed region — the same transitive copy eternal escape and an explicit
  capture-severing `CLOSE OVER` already perform
  ([lazy-closures.md § Lazy close](../../design/lazy-closures.md)) — severs
  its reach and re-derives a precise mask. The aggregate prices the operation
  up front: an empty aggregate means nothing to do, and each entry names a
  region the copy would free the claim on.
- **Frame discipline bounds the common case.** A short-lived frame freezes a
  small column; the pathology is a long-lived frame that held much and then
  sealed — measurable, once the tier exists, as aggregate-attributable
  occupancy.
- **Pre-seal narrowing is rejected.** Recomputing a tighter aggregate from
  actually-reachable values at seal time is exactly the storage scan
  atomicity deletes.

Pricing the copy-versus-pin choice gains a sealed term: the cost of holding
sealed region S is the storage of its aggregate's *transitive closure*,
computed by OR-folding aggregates — union dedups shared members within one
decision for free, and a closure whose slab bits have all converted is frozen
and only shrinks, so it memoizes without invalidation machinery. What the
union cannot fix is double-billing *across* candidate decisions: releases
whose closures share a sub-tier each claim the shared part, and the honest
marginal price of releasing S is only the uniquely-held slice of its closure.
That refinement — and the pressure model that consumes these prices, e.g. a
copy-versus-pin price that ramps with tier occupancy ahead of any hard cap —
remains design work, not an implementation detail.

## Locality tactics

These tactics keep the degenerate cases — row-zero reclamation, empty
aggregates, flat sealed records — common. All are heuristics: they exploit
shapes most programs exhibit most of the time, and none is a guarantee. One
line disciplines the absorptions among them: **an absorption is legal exactly
when it preserves its target's reach discipline.** A dying frame's per-value
masks are still maintained — seal step 1 rewrote them all its life — so it
may merge into a live region; sealed merges with sealed, where both sides
derive reach from aggregates. Only moving *already-sealed* storage back into
the live tier is barred: its per-value masks are dead bytes, and the severing
copy remains the lever there.

- **The crossing rule keeps frames out of the tier.** The general crossing
  tier of
  [destination-homed-construction.md](../../design/destination-homed-construction.md)
  — producer-born parts copy; references upward and sideways cross free —
  composes with the self rule: a caller-resident reference delivered back to
  its own region contributes the caller's own bit, which `mask & !bit(C)`
  erases at the mint. A per-call frame whose result crosses under the rule
  therefore delivers with no retained reach and reclaims at row-zero without
  sealing. This holds only where the rule is enforced and only for frames no
  reference escapes downward from — closures over locals and stored handles
  still seal, by design. Whether the rule applies at every adopt or only at
  yield deliveries is open per
  [yielding-iterators.md](../../roadmap/foundation/yielding-iterators.md).

- **Death-time absorption ties a dying frame to its unique live holder.**
  When N dies with `row(N) == bit(M)` and an empty naming set, N never
  seals: its chunks splice onto M's region, its frozen column ORs into
  `column[M]` through the standard `& !bit(M)` mint, and M's stored masks
  naming slot N are rewritten to nothing — the cross-region hold becomes a
  structural self-hold, which has no bits. That rewrite is the same bounded
  scan seal step 1 performs, and the trigger means M is the only frame
  scanned. No sealed id, no index entry, no accessor indirection: reads stay
  on the precise per-value-mask path, and future seal transitions maintain
  the absorbed masks automatically because the chunks are M's storage now.
  Retention is identical to sealing — a count-1 sealed region lives exactly
  until its holder's release anyway — so the record and the indirection are
  deleted without retaining more. Where it pays: a long-lived single
  consumer absorbs each kept per-iteration producer as it dies instead of
  confettiing the sealed tier with count-1 records, and a stash-first result
  — the case destination-homed construction's refusal rule deliberately
  declines — transfers wholesale without the delivery copy. Two guards. A
  loop cart must refuse it: old iterations dying uniquely held by the fresh
  cart would accrete without bound, and the one structural copy per tail hop
  ([tail-call-optimization.md](../../design/tail-call-optimization.md))
  exists precisely to keep per-iteration turnover. And absorption is
  wholesale where the crossing-rule copy is precise — dead locals ride
  along, permanently indistinguishable inside M — so a frame that stored
  much and delivers little should copy instead; region size is known, result
  size is not, a priced choice.

- **Seal-time absorption flattens ownership chains.** At P's seal, a sealed
  region S in P's hold set with holder count 1 is held by P alone — and mask
  validity plus aggregates-being-holds make P's record the only place in the
  system that names S, so absorbing S into P is purely local: OR S's
  aggregate into P's, splice S's storage chunks onto P's (pointer-stable),
  repoint S's reverse-naming-index entries. The candidate test is bitwise
  and bounded by P's own hold set — `count(S) == 1` per sealed entry (the
  live tier's analogue of the trigger is death-time absorption above). When
  call trees follow a single-consumer shape, chains collapse into flat
  records — one count, one folded aggregate, one storage bundle — and
  releasing one is a depth-one cascade; a program whose retained regions are
  genuinely shared absorbs nothing and keeps the full graph. For absorbed
  chains the uniquely-held slice of the pricing model is read directly off
  the merged record; the shared residue still poses the cross-decision
  double-billing question above. Generalizing absorption to regions held
  only within a group sealing together (a dying call subtree) is undecided —
  delimiting the group is design work — and carries a second payoff: holds
  internal to a group sealed as one record never manifest as
  sealed-naming-live edges at all; only the group's boundary reach survives
  the seal.

- **Seal-into-namer covers the downward direction.** When N dies with row
  zero and a singleton naming set {Q}, the sealed Q is provably N's only
  namer — the same mask-validity-plus-aggregates-are-holds argument as
  above, pointed the other way — so N seals *into* Q rather than minting a
  record: N's chunks splice onto Q's, N's frozen column ORs into Q's
  aggregate (registering Q under any slab bits new to it), and the rows N's
  column named trade bit N for Q in their naming sets. The work is the seal
  transition's own steps minus the record creation, and the test reads two
  structures the transition already holds: the row and the naming set.
  Together with seal-time absorption this covers both ends of an ownership
  chain; both remain sealed-tier-only merges, per the discipline line above.

## Failure direction

A reference count fails safe: a forgotten release leaks. This model fails
dangerous: a forgotten bit reclaims a live frame. That trade is accepted
deliberately, and it dictates the engineering posture: the matrix, the sealed
tier, and every hold transition are encapsulated behind a narrow interface
designed so that safe usage cannot skip a declaration — a value cannot be
stored without its mask passing through the mint OR, and a sealed region's
contents cannot be read except through the accessor that derives aggregate
reach — and the encapsulated core is tested exhaustively (property tests over
hold/seal/retire interleavings, plus the Miri slate) rather than audited by
convention.

## Layout

Row-major, so the liveness check is a contiguous row scan; the column clear
at reclamation and the freeze at seal stride, and the first cut accepts that
(chunk-aligned rows keep the stride cache-friendly). Per sealed region: one
hybrid aggregate mask, one holder count, one memoized closure mask at most.
Per slab slot: one sparse reverse-naming set. A per-row holder count
maintained as derived data — derived *from* attributed transitions, never a
free-standing count — is a measurable later optimization; it is an
implementation detail invisible to the interface either way.

## Open work

Everything. No part of this design is implemented, and the roadmap sequence
that ships it — pool-owned frames and handle-based anchors, the slab matrix,
the sealed tier and its accessor, the lexical structure, hybrid reach
carriers, the seal transition, the admission interface, the pricing and
pressure model, the recycling gate — is not yet planned. The koan-side
primitive the consolidation gate fires — the transitive callable copy, at the
priced escape seam and at an explicit capture-severing `CLOSE OVER` — is
shipped ([lazy-closures.md § Lazy close](../../design/lazy-closures.md)); one
further primitive it leans on is tracked:

- [Yielding iterators](../../roadmap/foundation/yielding-iterators.md) —
  node-backed producers that yield many values before dying, the surface
  family the admission iterator belongs to.
