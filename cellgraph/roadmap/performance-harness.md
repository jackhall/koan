# Performance measurement harness

A benchmark set over the substrate's verbs, a recorded trend log, and the first
round of scan and allocation removals landed under it.

**Problem.** The crate has no performance measurement. `seal_work` and the
`Merges` tally ([table.rs](../src/table.rs)) count units of maintenance for
the bounded-transition tests, but nothing reads allocation counts or time for
a whole verb, so a change that adds a scan or an allocation to a per-step path
lands unnoticed. Several such costs are in the code today, each on a path that
runs per verb rather than per merge:

- `settle` walks every slab slot on every `release` and repeats until a pass
  makes no progress, and `disposable` is a column scan over `occupied()` —
  itself an O(cap) filter — for each dead slot per pass. Both are bookkeeping
  the slab can maintain instead: a dead-slot worklist, and a per-slot count of
  birth rows naming it (birth rows are written only at `create` and cleared
  wholesale at `release`, so the count is exact).
- `dispose` scans the pin column the same way, including on the reclaim path
  where only "zero holders" is needed.
- `is_empty` walks the slab when `free.len() == cap` answers in O(1).
- `holds_of` allocates a `Vec` per visited node of every pricing walk; both
  callers immediately extend a stack with it.
- `pin_price` copies the pin row, clones the sealed-hold set, and collects
  seeds before walking, even when the reach is already covered by the
  destination and the answer is zero — the common case of placing a value
  into a cell that already holds its home. An always-pin embedder pays the
  whole walk per operand.
- The memo hit in `reached_from_record` clones the record list that
  `bytes_of` only sums.
- `SealedSet::union_with` is a per-id binary search plus `Vec::insert`,
  O(m × n) with shifting, where the type's doc says union is a merge.
- `retire_record`, `seal`, and `fold_into_record` collect slab slots into a
  `Vec` before touching `naming`, though the mask in hand is a local or a
  borrow of the disjoint `sealed` field; `retire_record` also clones the
  sealed half of an aggregate it owns by value.

**Acceptance criteria.**

- A benchmark set exercises the substrate over a range of shapes, each sized
  to the smallest `n` that shows its trend: a keep-and-redeem loop in one cell
  (per-step value cost); a push chain of single-consumer producers built into
  their consumer and released (death-time absorption); a pull chain of holds
  read after the producer seals (seal transition and record retirement); a
  deep birth chain created and released innermost-first (`settle` over a
  parent stack); a fan-out placement over many operands (the pricing walk);
  and a shared sub-tier held from two branches, wound down (sealed-tier
  cascade).
- Each benchmark reports an allocation count, allocated bytes, and wall time
  per verb, from a debug build like every other measurement in the repo, with
  the count and bytes deterministic across runs.
- One command sweeps the set and prints a delta against the newest recorded
  commit; with a record flag it appends this commit's readings to
  `cellgraph/observe/perf.csv`, a committed tidy dataframe — one row per
  `(date, sha, dirty, benchmark, n, cap, verb)` carrying `calls`,
  `allocations`, `bytes`, `nanos` — so a reading is analysed with
  `pandas.read_csv` and a pivot rather than by eye. Re-recording at a commit
  replaces that commit's rows, the file keeps the three most recently
  recorded commits, and it is marked `-diff` in `.gitattributes` like the
  coverage record.
- `settle` visits only dead slots, `disposable` is O(1) against a maintained
  birth-holder count, `dispose` reaches `reclaim` without a column scan,
  `is_empty` is O(1) on the slab half, `holds_of` pushes onto the caller's
  stack, `pin_price` returns zero without allocating when the reach is inside
  what the destination already holds, `SealedSet::union_with` is a linear
  merge, and the collect-then-iterate and clone-of-owned sites above are
  direct loops and moves.
- The recorded row after those changes shows allocation count and bytes no
  higher than the row before on every benchmark and lower on the ones the
  changes target, and wall time no higher on any; the existing lib tests,
  `tests/surface.rs`, and the crate's Miri slate
  ([observe/miri_slate.md](../observe/miri_slate.md)) still pass.

**Directions.**

- *Harness shape — open.* (a) `criterion` as a dev-dependency under
  `cellgraph/benches/`; (b) a hand-rolled `#[test]`-style runner behind a
  feature, timing with `Instant` and counting through a delegating allocator
  in the shape of [audit/counting_alloc.rs](../../audit/counting_alloc.rs),
  driven by a Python sweep like
  [tools/alloc_audit.py](../../tools/alloc_audit.py). Recommended: (b); the
  allocation count is the reading that gates, it is deterministic, and it
  keeps the crate's dependency list to `bumpalo`.
- *Which reading gates — decided.* Allocation count and bytes gate a change;
  wall time is reported for the trend and never asserted, since a
  low-memory development machine makes it noisy.
- *Record tracking — decided.* `observe/perf.csv` is committed, so a reader
  learns what HEAD costs without re-measuring a base revision, and the
  history is there to analyse.
- *Record history — decided.* Three commits, not five: the dataframe is
  there because reasoning about performance is varied, not for history, and
  older rows remain in git.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Per-step scratch region](step-scratch-region.md) — its allocation removals
  are asserted against this harness.
- [Compile-time slab width](compile-time-slab-width.md) — its mask and matrix
  changes are asserted against this harness.
