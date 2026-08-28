# Liveness matrix

Liveness bookkeeping for pool-resident frames, expressed as attributed bit
matrices rather than reference counts. This design is aspirational: nothing in
it is implemented, and it supersedes the reference-counted holds that
[reach.md](reach.md) and the scheduler's anchor plumbing describe today.

## The model

Frames live in a bounded pool owned by the node store. A frame's identity
outside the pool is a **handle** — pool slot index plus generation — never an
owning pointer; the pool is the sole owner of frame memory, which is what makes
reset-in-place recycling trivial rather than gated.

Liveness is a bit matrix over pool slots. Bit (N, M) means *frame M keeps
frame N alive*. The diagonal bit (N, N) means *something is currently
executing in frame N* — the drain marks it for the duration of a step instead
of holding a clone. In the row-major layout, frame N's liveness is one
contiguous row: **N is reclaimable exactly when row N is zero.** There is no
other gate — no count arithmetic, no census of structural self-holds (a
frame's internal references to its own storage have no bits), no
drop-ordering discipline in the drain loop.

The matrix is **square, indexed by slot**. Frames are well-ordered by creation
order, which is what guarantees the hold graph is acyclic — but slot reuse
means slot index does not encode that order, so acyclicity is a dynamic
invariant over the generations of live occupants, not a property of the
matrix's shape. A triangular encoding would require monotone frame ids and
unbounded growth; the square form resizes only when the pool itself grows or
shrinks.

## Two relations, two structures

Holds arise from two sources with different disciplines, and each gets its own
structure rather than sharing one matrix:

- **Pin holds** — value reach. Frame M's region stores values whose borrows
  reach other frames' storage; each such frame gets a bit in M's column. These
  accumulate throughout M's life.
- **Lexical holds** — chain topology. A frame keeps its lexical outer chain
  alive. These are fixed at frame birth (a frame's lexical hold set is its
  outer's set plus the outer's own bit — the cactus chain as masks) and never
  change afterward.

Keeping them separate lets each structure assert its own discipline: lexical
bits are written once at birth and are immutable; pin bits are
monotone-growing. It also leaves the lexical side free to take a sparser shape
than a matrix — lexical relationships are sparse, and their representation is
deliberately undecided here. A frame is reclaimable when it is dead in *both*
relations.

## Reach as a slot bitmask

A value's reach — the set of foreign regions its borrows keep alive — is a
bitmask over pool slots. This replaces the owned pin-bundle antichain and the
machinery that exists to keep antichains small:

- **Union and dedup are `OR`.** Composing reach for nested closures and
  collections is a bitwise OR of member masks; deduplication is free because
  OR is idempotent.
- **Mint is a retention, literally.** Storing a value into frame M's region
  performs `column[M] |= mask` — one vectorized OR. The destination's column
  *is* the retained reach.
- **The self rule is an AND-NOT.** A region must not hold itself alive
  (bit (M, M) is the executing flag, and a self-hold would be an
  unreclaimable cycle), so the mint masks off the destination's own bit:
  `column[M] |= mask & !bit(M)`.
- **The eternal rule disappears.** Storage that outlives every region simply
  owns no pool slot and contributes no bit.
- **Subsumption disappears.** Antichain minimization pays for itself only when
  each member costs an owning pointer; bits cost nothing, so reach is never
  folded, and the per-member pin semantics hook (`PinsRegion`) and its unsafe
  contract go with it.

## Invariants

The model is sound on a chain of invariants that must hold together:

1. **Monotone holds.** A frame's hold set (its column) only grows during its
   life. The sole release is the whole-column clear at the holder's death —
   there is no mid-life, per-reason release. This is what makes bit-setting
   idempotence safe: overlapping reasons for the same bit can never
   desynchronize, because nothing clears a single bit.
2. **Mask validity.** A mask bit is valid while its slot's occupant lives; the
   occupant lives while any live column holds its bit; a value's mask is
   always covered by its host region's column (the mint OR establishes this);
   and a value only moves between regions while its current host is live, so
   the OR into the new column happens while every bit in the mask is still
   valid.
3. **Retirement cascade.** When frame M dies, its column clears. Any frame M
   alone was keeping alive now has a zero row; the dying frame's own hold mask
   names exactly the rows worth re-checking, so retirement detection walks
   that mask, not the pool. Because reach masks carry transitive coverage (a
   value that reaches N through M's chain has N's bit directly), a cascade
   never releases a frame some survivor still needs.
4. **Fresh generations.** A reclaimed slot's next occupant carries a new
   generation; stale handles are detectable, never silently re-bound.

## Why masks cannot go stale

Reach masks name slots by index and slots are reused, so a mask stored beside a
value looks like it could dangle. It cannot, and the design carries no per-bit
version stamp, because staleness is prevented structurally rather than
detected. The loop closes through the invariants above: a mask is readable only
through a live holder; the covering clause of invariant 2 puts every bit of a
readable mask into some live column; column M holding bit N makes row N
nonzero; and a zero row is reclamation's *only* gate. So no slot named by a
live mask can be recycled — and when the holder dies, its masks become
unreadable in the same stroke that clears its column. Generations exist to make
a stale *handle* detectable; masks never need them.

What the argument demands in exchange: **every habitat a mask can live in must
be covered by a column.**

- **Region-resident values** — covered by the host's column via the mint OR.
  By construction.
- **Step transients** — covered by the executing frame's diagonal bit for the
  duration of the step. By construction.
- **In-flight deliveries** — a produced value awaiting delivery at finalize
  sits in neither habitat. Either the producer frame stays live until the
  delivery OR lands in the consumer's column, or the scheduler owns a
  pseudo-column for the in-flight band. Undecided.
- **Eternal storage** — masks there are empty: escape severs reach, consistent
  with eternal storage owning no bit.

## Pool geometry

Growth appends slots, so indices are stable and an old mask read at a wider
pool width zero-extends — a zero bit means "no reach", which is always sound.
The pool never compacts: renumbering live slots would mean rewriting every
stored mask, so shrinking may only drop a free *tail* of slots. With a bounded
pool, masks are fixed-width at the cap and even the width question disappears.

## Bounding the pool

A slot stays occupied while its row is nonzero, so occupancy decomposes into
**executing frames** plus **retained regions** — dead frames whose storage some
survivor still reaches. The two populations are bounded by different means.

The executing population is bounded by admission control: rather than the
embedder eagerly scheduling every node, it hands the scheduler an iterator of
node creations, and the scheduler consumes it against its current resources.
The dependency direction survives — workgraph names no embedder type, so a
lazy, embedder-supplied generator is a clean interface.

The retained population is program-dependent — a program building a deep
cactus of closures retains regions no matter how slowly nodes are admitted —
so admission alone cannot bound it. The matrix is the oracle for the
consolidation lever: when frame N dies with a nonzero row, row N names exactly
the frames still reaching it, so the copy-versus-pin choice at frame death is
informed rather than blind, and the per-value masks bound the scan — only
values in row-named frames whose mask contains bit N are candidates to copy
out.

Forced consolidation at a hard cap is **not** a sufficient pressure valve on
its own: nearly every practical program would eventually hit the cap and fall
off a performance cliff of copies. The pressure model is open. Candidate
shapes: a copy-versus-pin price that rises with pool occupancy, so
consolidation pressure ramps smoothly ahead of the cap; or an Rc-managed side
table of zombie regions — dead-frame, still-reached storage evicted from the
pool to free its slot. The side-table candidate has design work of its own:
eviction leaves live masks naming the freed slot, so it must redirect those
bits or keep the slot reserved, or the staleness argument above breaks. In
every variant, the copy-or-pin decision itself is expected to be more complex
than a row-sparsity heuristic; deciding it is design work, not an
implementation detail.

Consolidation chances can also be created ahead of frame death. A koan
`CLOSE OVER` form builds a closure whose capture set is *copied* into the
closure's own region rather than pinned. Severing requires a deep copy — a
shallow copy leaves borrows pointing where they pointed and severs nothing —
and the mask prices the operation up front: an empty mask means nothing to do,
and each bit names a region to pull from. It is the same operation eternal
escape already performs, generalized into a surface the program (or a policy)
can invoke deliberately.

## Failure direction

A reference count fails safe: a forgotten release leaks. This model fails
dangerous: a forgotten bit reclaims a live frame. That trade is accepted
deliberately, and it dictates the engineering posture: the matrices and every
bit transition are encapsulated behind a narrow interface designed so that
safe usage cannot skip a declaration — a value cannot be stored without its
mask passing through the mint OR — and the encapsulated core is tested
exhaustively (property tests over hold/retire interleavings, plus the Miri
slate) rather than audited by convention.

## Layout

Row-major, so the liveness check is a contiguous row scan; the column clear at
death strides, and the first cut accepts that (chunk-aligned rows keep the
stride cache-friendly). A per-row holder count maintained as derived data —
derived *from* attributed bit transitions, never a free-standing count — is a
measurable later optimization; it is an implementation detail invisible to the
interface either way.

## Open work

Everything. No part of this design is implemented, and the roadmap sequence
that ships it — pool-owned frames and handle-based anchors, the pin matrix,
the lexical structure, reach-mask carriers, the admission interface, the
retention pressure valve, the recycling gate — is not yet planned. Two
koan-side primitives the design leans on are tracked:

- [CLOSE OVER](../../roadmap/foundation/close-over.md) — the capture-severing
  copy that creates consolidation chances ahead of frame death — and
  [lazy close](../../roadmap/foundation/lazy-close.md), the transitive
  callable copy this design's consolidation gate actually fires.
- [Yielding iterators](../../roadmap/foundation/yielding-iterators.md) —
  node-backed producers that yield many values before dying, the surface
  family the admission iterator belongs to.
