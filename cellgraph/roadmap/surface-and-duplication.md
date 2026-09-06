# Public surface and internal duplication

The substrate exports more than any embedder has asked for, and writes several
of its own primitives more than once.

**Problem.** `cellgraph` exports 21 types and 44 public functions with no
consumer: nothing in koan's [`src/`](../../src) or in
[`workgraph/`](../../workgraph) imports the crate, and
[adopt-cellgraph.md](../../workgraph/roadmap/adopt-cellgraph.md) names only
`occupancy()` while leaving the pressure model open. Parts of that surface are
unreachable, duplicated, or profile-dependent.

- The price queries — `closure`, `unique_closures`, `sealed_retained_bytes` —
  each take a `SealedId` an embedder has almost no way to hold. Every test in
  the crate reaches ids through the crate-private `sealed.ids()` or
  `sealed_holds[..]`; the one public route is `continuation()`'s derived reach,
  and only where the stored continuation mask happened to name the slot that
  sealed. `occupancy()` reports a record count an embedder cannot then name.
  `Mask`, `SealedId`, `SealedSet` and `Closure` are public only to feed these
  three queries, and `Sealed::reach` / `Opened::reach` only to return a `Mask`.
- `closure(id)` returns exactly `unique_closures(&[id])[0]`.
- `HoldNode`, `debug_ring_from` and `debug_ring_from_sealed` are
  `#[cfg(debug_assertions)]`, so the public API changes shape with the build
  profile: code naming them compiles in dev and fails in release, and
  `CellTable::is_empty`'s doc link to `debug_ring_from_sealed` dangles there.
- `Mask::names` and `Mask::slab_slots` index a boxed slice with a
  caller-supplied slot and cap and panic out of range, with no stated
  precondition; `slab_slots`'s `cap` argument repeats a width the mask carries.
- `SealedSet` derives `Default` and `StaleHandle`'s field is `pub`, so each has
  a public constructor while `SealedSet::new` and `Handle::new` are
  `pub(crate)`.
- `pub mod reattach` alongside the root re-exports gives every reattach item two
  public paths, the `reattachable!` macro included.
- `Erased::store`, `Erased::erase` and the unsafe `Erased::reattach` are public
  though only the crate calls them; the type needs to be public only for the
  `Erased<V>: Copy` bounds the doors take.
- The same word-and-bit arithmetic is written three times, across
  `Mask::{add, names, remove_slot}`, `BitRow::{set, clear, test}` and
  `Matrix::place`. Within `Mask`, `union_with` repeats `union_slab_with`'s loop
  and `replace_slot` repeats `remove_slot`'s body.
- `seal`, `absorb_into_cell` and `seal_into_namer` each build the dying cell's
  frozen hold set with the same `Mask::with_words` over `pins.row_words(slot)`
  and a taken `sealed_holds[slot]`, and the latter two share the `clear_row` /
  `recycle` / `release_sealed_holds` tail.
- `seal_into_namer` mints no record and takes no id — it folds a dying cell into
  an existing record, which is the tier-crossing twin of `absorb_into_cell` —
  but its name reads as a third kind of seal.
- The two `holders -= 1` sites in `table.rs` carry no assertion of the
  `holders >= 1` precondition they rely on, though
  [`properties.rs`](../src/table/tests/properties.rs) checks it of the tier.

**Acceptance criteria.**

- Embedders see none of the reach or price model. `Mask`, `SealedId`,
  `SealedSet`, `Closure`, `Occupancy` and `Mark` are not nameable from outside
  the crate; `unique_closures`, `sealed_retained_bytes`, `occupancy`,
  `region_bytes`, `mark` and `absorbed_since` are `pub(crate)`;
  `Sealed::reach` is `pub(crate)` and `Opened` carries no reach. A price
  returns to the embedder only through
  [resident-carriers.md](resident-carriers.md)'s closure.
- One closure-price entry point exists, and the tests price a single record
  through it.
- The crate's public API is identical under `--release` and under a dev
  profile, and an integration test under `tests/` that exercises every
  exported item compiles and passes under both.
- `Erased::store`, `Erased::erase` and `Erased::reattach` are `pub(crate)`; no
  public `unsafe fn` remains.
- One bit-set type backs the slab half of a reach mask, the executing row, and
  a row of each relation matrix; no word-and-bit arithmetic is written twice.
- One helper builds a dying cell's frozen hold set, called from every seal and
  absorb path, and the two tier-crossing absorptions share their common tail.
- The absorption that folds a dying cell into an existing record is named for
  folding rather than for sealing.
- `SealedSet` and `StaleHandle` have no public constructor.
- Each item of the reattach seam has exactly one public path.
- Both `holders` decrements carry a `debug_assert!` of the precondition, and
  every slot query on a reach mask debug-asserts its index against the mask's
  own width.
- `CellTable::new`'s documentation states that slab relations cost
  `cap × ceil(cap / 64)` words apiece.
- [design/liveness-matrix.md](../design/liveness-matrix.md) and
  [README.md](../README.md) describe the price queries at the visibility they
  ship at.

**Directions.**

- *Scope of the narrowing — decided.* Every price query goes `pub(crate)`,
  and so does everything that exists only to feed one. An embedder is owed
  exactly one price, at a crossing, through the verdict closure of
  [resident-carriers.md](resident-carriers.md); it is never owed a mask, an
  id, or a tier count. Widening later is not a breaking change and narrowing
  later is, and the crate is `publish = false` with no consumer.
- *Which closure query survives — decided.* `unique_closures`. `closure(id)` is
  its one-element specialization, while marginal pricing cannot be rebuilt from
  the single-candidate answer.
- *Carrier reach — decided.* `Sealed::reach` is `pub(crate)`, since the mints
  read it. `Opened` drops its reach field and `derive_reach` goes with it:
  nothing in the crate consumes a read-out value's reach, and a `pub(crate)`
  item only tests call is dead code. The derivation returns where it is
  consumed, the redeem door of [resident-carriers.md](resident-carriers.md).
- *The profile-dependent diagnostics — decided.* `HoldNode` and both ring
  walks become `#[cfg(test)]`: `debug_ring_from_sealed` takes a now-private
  `SealedId`, and a `pub(crate)` walk only tests call is dead code in every
  other build. `is_empty()` is the embedder's only alarm.
- *Bit-set unification — decided.* One bit-set type, generic over its word
  storage: owned for a reach mask's dense half and the executing row, a
  borrowed slice view for a matrix row. The matrix stays one flat allocation,
  so the reclaim query's scan across rows keeps its stride, and the one
  `place` computation gives every slot query its width to assert against. A
  boxed row per slot would cost that scan a pointer load per row.
- *Underflow guards — decided.* `debug_assert!` only, at both sites. The
  decrementing entity is a holder of its target by construction at each, and
  [`properties.rs`](../src/table/tests/properties.rs) already asserts
  `holders >= 1` and holder-count agreement across generated interleavings, so
  a saturating or checked decrement would encode a bookkeeping bug as policy.
- *Slab cost — decided.* A documentation note on `CellTable::new`. A dense
  liveness matrix is quadratic in the cap by construction, and the sparse
  alternatives cost the contiguous row the seal transition's word copy needs.
- *Generation wraparound — deferred* to this roadmap's
  [unplanned work](README.md#unplanned-work).

## Dependencies

Best done before [adopt-cellgraph.md](../../workgraph/roadmap/adopt-cellgraph.md)
begins, since narrowing a surface is a breaking change once a consumer names it
— an ordering preference, not a prerequisite either way.

**Requires:** none — the crate stands, and this item only reshapes what it
already ships.

**Unblocks:**

- [Resident carriers and the crossing price](resident-carriers.md) — the
  reach vocabulary is private before a door is added over it.
