# Per-step scratch region

**Problem.** The table has no scratch storage. Every transient a verb builds
— a worklist, a holder list, the views a build closure receives — is a fresh
`Vec` that lives for one call and returns to the allocator, so the per-verb
allocation count grows with the number of transients rather than staying at
zero. Koan's own step loop already homes transients on a scratch arena reset at
every drain pop rather than on the frame, and the substrate is the layer
underneath it. The transients on the verb paths in [table.rs](../src/table.rs)
today:

- `cross` collects `(Erased, Verdict)` pairs, and `crossed_views` collects
  the re-anchored views from them: one heap vector per placement over
  operands, in `alloc_into` and `store_successor_capturing`, sized by the
  operand count.
- `pin_price` clones the destination's sealed-hold set as the walk's seen set
  and collects the reach's sealed ids as its worklist; `walk` grows that
  worklist; `prime_memo` builds a one-id worklist and a record list it
  discards whenever the closure is not frozen.
- `dispose` collects the holder list; `take_lineage` and `migrate_residents`
  each build a list of the handles they move.
- `fold_into_record` collects the transferred ids and the duplicated ids;
  `absorb_singletons` and `release_sealed_holds` each build a pending
  worklist; `absorb_into_cell` builds a duplicate set; `absorb_singletons`
  also collects the absorbed aggregate's slab slots before touching `naming`,
  though the aggregate is an owned local.

The test-only walkers — `Reached`, `reached_from`, `reached_from_record`,
`unique_closures`, `holds_of`, and the ring walk — allocate too, but run in no
verb and in no benchmark.

**Acceptance criteria.**

- The table owns one scratch region, reset at the entry of every verb
  (`create`, `enter`, `release`) and never inside one, and every transient
  listed above lives in it; none of those sites calls the global allocator on
  a table whose scratch chunk holds the verb's transients, which it does from
  construction for every harness shape.
- A step's build closure receives its views from the scratch region, and the
  views still cannot outlive the build call — pinned as a compile-fail
  doctest beside the existing severed-view one.
- The harness's keep-and-redeem, fan-out, and push-chain benchmarks record an
  allocation count per verb that does not grow with the operand count or the
  worklist depth, and no benchmark records a higher count or more bytes than
  the row before (`tools/cellgraph_perf.py --gate`).
- No benchmark row is slower than the baseline rebuilt and run beside it by
  more than the sweep tool's tolerance and that row's own trial noise
  (`tools/cellgraph_perf.py --gate-time`, below).
- The crate's Miri slate is clean with the region in place.

**Directions.**

- *Storage — decided.* One `bumpalo::Bump` behind a crate-private `Scratch`
  wrapper, with bumpalo's `collections` feature for a bump-backed `Vec`; every
  transient, worklists included, is a scratch vector or a scratch-backed sorted
  id set. Not persistent `Vec` fields: the cascade's worklists nest, so those
  would need one field per nesting level and a take-and-restore at every use,
  and the views need a bump regardless. The seen set comes from making the
  sorted id set generic over its buffer rather than writing a second one.
- *First chunk — decided.* Sized at construction, which no benchmark row
  meters, like the slab itself. A lazily minted chunk would raise the small
  sizes' byte readings above the per-operand vectors it replaces, and the
  chunk survives every reset.
- *Reaching the scratch — decided.* The cascade takes `&mut self`, so the
  scratch is taken off the table for the length of a verb — parked on the
  step context under `enter`, restored by its `Drop` — and passed to every
  method that builds a transient as a parameter. Nothing reads the table's
  scratch field during a verb.
- *Reset point — decided.* Verb entry, not step exit: a `release` runs its
  cascade outside any step, and the region must be empty when it starts.
- *Removal over relocation — decided.* A collect-then-iterate site whose
  source is an owned local is a direct loop, not a scratch list: the absorbed
  aggregate's slab slots in `absorb_singletons`. A move that allocates
  nothing — `Residents::take` in `migrate_residents` — is left alone.
  Scratch is for transients that must be materialised.
- *Time gate — decided.* Wall time gates this item under a tolerance and a
  noise bound, never as an absolute comparison: a row regresses only when its
  fastest trial exceeds the rebuilt baseline's fastest by more than 10 %, the
  excess exceeds that row's own noise (the baseline's median trial minus its
  fastest, in the same sweep), and the baseline row is at least 5 µs. The
  sweep tool gains a `--gate-time` flag carrying that rule; it gates only
  beside a rebuilt baseline, and the plain `--gate` keeps the harness item's
  allocations-and-bytes-only ruling.
- *Test-only walkers — decided.* Out of scope; they keep their heap vectors
  and change only where `walk`'s signature moves them onto the parked
  scratch.

## Dependencies

**Requires:** none — the allocation-count criteria are read off
[tools/cellgraph_perf.py](../../tools/cellgraph_perf.py) against the record in
[observe/perf.csv](../observe/perf.csv).

**Unblocks:** none — leaf.
