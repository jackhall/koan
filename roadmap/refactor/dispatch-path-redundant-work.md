# Redundant work on the dispatch path

Three removals on the per-dispatch path that each delete work outright — no new
staging substrate, no shared plumbing.

**Problem.** Three sites on the hot dispatch path do work whose result is already
known, discarded, or identical to what the caller holds:

- **A re-splice that changes nothing.** `walk_and_invoke`
  ([src/machine/execute/decide/keyworded.rs](../../src/machine/execute/decide/keyworded.rs))
  always rebuilds: `part_walk` allocates a `new_parts` vector, then `respliced`
  bump-copies it, re-bumps `stored_untyped_key`, and recomputes `classify_dispatch_shape`
  and `operator_probe_for`. For a call with no wrap slots and no eager parts — `PRINT "hi"`
  — every byte of that is identical to what the node already holds.
- **A bucket read taken to look at one `Option`.** `pending_operator_sources`
  ([src/machine/execute/decide/operator_chain.rs](../../src/machine/execute/decide/operator_chain.rs))
  calls `lookup_function_stored(key, cutoff).pending` for two keys × every ancestor
  scope × every distinct operator in a chain. Each call builds
  `FunctionLookup`'s full filtered, visibility-checked `overloads` vector
  ([src/machine/core/bindings.rs](../../src/machine/core/bindings.rs)) and drops it unread.
- **Construction doors that take an owned run they immediately copy.**
  `WorkingExpression::build` ([src/machine/model/ast/working.rs](../../src/machine/model/ast/working.rs))
  opens with `brand.allocator().slice(&parts)`, and `synthesized` additionally reads
  `parts_extent(&parts)`. Both want `&[T]`; `BumpAllocator::slice` already takes `&[T]`.
  Every `new`/`synthesized`/`build` call site heap-allocates a `Vec` purely to hand it to
  that copy. Roughly eighteen of those sites pass a fixed-length run, and five sit inside
  the operator-chain fold loops (`reduce_fold_left`, `reduce_fold_right`,
  `install_pairwise_fold`), so an n-operator chain pays n of them.

**Acceptance criteria.**

- A dispatch whose part walk staged no eager sub and spliced no wrap slot reuses the
  node it was given: no `new_parts` vector, no bump re-copy of the parts run, no
  re-bumped untyped key, and no recomputed `shape` / `operator_probe`.
- `Bindings` answers a pending-only probe without materializing the bucket's visible
  finalized overloads, and `pending_operator_sources` uses it.
- `WorkingExpression`'s construction doors take `&[Spanned<WorkingPart<'a>>]`, and every
  fixed-length call site passes a stack array, allocating nothing on the heap.
- A call site whose run length is computed builds into the region bump directly rather
  than staging through an owned `Vec` first.
- The recorded operator-chain allocation count drops, and the two removals that are
  shape-invariant (the re-splice skip, the pending probe) leave every existing dispatch
  test passing unchanged.

**Directions.**

- *Skip predicate — decided.* `wrap_set.is_empty() && staged_subs.is_empty()` is the
  condition under which the walk provably produced the run it was handed; return the
  original node on that path.
- *Computed-length runs — open.* Add a `slice_from_iter`-style verb to `BumpAllocator`
  (bumpalo offers `alloc_slice_fill_iter`), versus staging those runs on a scratch buffer.
  Recommended: the bump verb, since the run is region-destined anyway and a scratch
  staging buffer would only add a second copy. Affects `from_ast`, `part_walk`, the
  `respliced` callers, [src/machine/core/kfunction.rs](../../src/machine/core/kfunction.rs)
  and [src/machine/model/types/typed_field_list.rs](../../src/machine/model/types/typed_field_list.rs).
- *Pending-probe surface — open.* A `pending_only` method on `Bindings` versus making
  `FunctionLookup`'s `overloads` lazy. Recommended: the dedicated method — the lazy form
  would have to hold the `tables` borrow across the candidate walk, which the current
  copy-out exists to avoid.

## Dependencies

**Requires:** none.

**Unblocks:** none.
