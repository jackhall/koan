# Per-step scratch region

**Problem.** The table has no scratch storage. Every transient a verb builds
— a worklist, a seed set, the views a build closure receives — is a fresh
`Vec` that lives for one call and returns to the allocator, so the per-verb
allocation count grows with the number of transients rather than staying at
zero. Koan's own step loop already homes transients on a scratch arena reset at
every drain pop rather than on the frame, and the substrate is the layer
underneath it. The transients in [table.rs](../src/table.rs) today:

- `cross` collects `(Erased, Verdict)` pairs, and `crossed_views` collects
  the re-anchored views from them: two `Vec`s per placement over operands,
  in `alloc_into` and `store_successor_capturing`.
- `pin_price` collects its seeds; `walk` allocates its stack and the two
  `Reached` vectors; `reached_from_record` allocates a `Reached` for a memo
  hit. One set per priced operand.
- `dispose` collects the holder list; `take_lineage` builds a `Vec` for the
  handles it moves; `migrate_residents` takes the moved block as a `Vec`.
- `fold_into_record` collects the transferred ids; `absorb_singletons` and
  `release_sealed_holds` each build a pending worklist; `absorb_into_cell`
  builds a duplicate set.
- `unique_closures` (test-only) allocates a walk list, two count tables, and
  a priced list per query.

**Acceptance criteria.**

- The table owns one scratch region, reset at the entry of every verb
  (`create`, `enter`, `release`), and every transient listed above that
  survives the harness's first round lives in it; none of those sites calls
  the global allocator on a warm table.
- A step's build closure receives its views from the scratch region, and the
  views still cannot outlive the build call.
- The harness's keep-and-redeem, fan-out, and push-chain benchmarks record an
  allocation count per verb that does not grow with the operand count or the
  worklist depth, and no benchmark records a higher count, more bytes, or
  more time than the row before.
- The crate's Miri slate is clean with the region in place.

**Directions.**

- *Storage — open.* (a) a `bumpalo::Bump` with `reset()`, taking bumpalo's
  `collections` feature for a bump-backed `Vec`; (b) persistent `Vec` fields
  on the table, cleared per use, so capacity is kept across verbs with no new
  feature and no lifetime on the buffer. Recommended: (b) for the worklists
  and holder lists, which have one owner each; (a) for the views and seeds,
  which are handed to a closure at a brand and want the `'r` a bump borrow
  already gives.
- *Reset point — decided.* Verb entry, not step exit: a `release` runs
  `settle` outside any step, and the region must be empty when it starts.
- *Removal over relocation — decided.* An allocation the harness item
  already removes outright (the `holds_of` vectors, the collect-then-iterate
  sites) is not moved into scratch; scratch is for transients that must be
  materialised.

## Dependencies

**Requires:**

- [Performance measurement harness](performance-harness.md) — the
  allocation-count criteria are read off its trend log.

**Unblocks:** none — leaf.
