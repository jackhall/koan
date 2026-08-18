# Relocate a source run in one act

Every site that builds one value out of many delivered ones folds them in pairwise, and
each step re-bumps the run gathered so far. Give the relocation verbs an N-ary door and
the shape disappears.

**Problem.** Three folds in the relocation path are quadratic in element count, in both
heap copies and region bytes:

- `fold_cells` (`src/machine/execute/decide/literal.rs:105-141`) — every aggregate literal
  (`[…]`, `{…}`, record).
- The `RecordNewType` field fold (`src/machine/execute/decide/constructors.rs:490-510`) —
  bounded by record arity.
- `fold_dep_view`, `StepContext::alloc_with`'s own per-dep fold
  (`workgraph/src/witnessed/step_ctx.rs`) — bounded by dep count.

All three run the same closure: `Vec::with_capacity(n + 1)`, `extend_from_slice` the
accumulated run, push the new element, hand the whole thing to `allocator().slice(…)`. For
N elements that is N heap `Vec`s of mean length N/2 *and* N bump slices of mean length N/2
— region bytes a bump cannot reclaim until the frame dies, since only its most recent
allocation is reusable. Measured RSS over the interpreter floor for a list literal: 12 MB
at n=500, 48 MB at n=1000, 190 MB at n=2000.

The rebuild is not gratuitous. `Delivered::transfer_into` bounds its destination family
`B: Reattachable + DropFree`, so an owned `Vec` cannot ride the accumulator between fold
steps, and each step opens a fresh `'b`, so no buffer named outside a step can receive a
value built inside one. Staging the run outside the region — the obvious fix — does not
typecheck. Whatever replaces the rebuild has to be region-resident and `Copy`, or the fold
shape itself has to change.

**Acceptance criteria.**

- A relocation door takes N sources and one destination, re-anchors the whole source run at
  a single brand, and builds the product in one pass — no accumulator between steps.
- `fold_cells`, the `RecordNewType` field fold, and `alloc_with`'s dep fold each stage their
  sources once and bump the product run exactly once; no `extend_from_slice`-then-push
  rebuild survives in the relocation path.
- Region bytes and heap copies for an N-element aggregate are linear in N: list-literal RSS
  over the interpreter floor stays within a small constant multiple of the aggregate's own
  size at n=500, 1000 and 2000.
- Fixed cost at small N does not regress: allocations per two-element aggregate are measured
  and are no higher than the pairwise path's.
- Misalignment between a source and the product cell it produced is not expressible — the
  retention predicate receives a source and its own product cell together, rather than an
  index into a run it must trust. Where the plumbing forbids that, the alignment contract is
  stated on the door and a debug assert checks the run lengths.
- A mixed-run escape test asserts exactly which producers survive: one run holding both a
  plain-data cell (releases its producer) and a still-borrowing closure cell (materializes
  its host), ordered so that a permuted or off-by-one claim fails it.
- No caller allocates purely to adapt to the door's signature — the aggregate sites pass the
  run they already hold.
- The coverage each cell's rebuild proves its holder rule against is stated at the site,
  including whether it is the cell's own or the union across the run, with the soundness
  argument written down.
- `workgraph/design/witnessed-memory.md`, `design/scheduler-library.md` and
  `step_ctx.rs`'s module header describe the N-ary verb; nothing still calls
  `merge_composed` the single engine behind the relocation verbs, or `alloc_with` a fold of
  dep envelopes.
- `tools/verify.sh` is green, the koan and workgraph test suites pass, and the Miri slate
  reports zero leaks and zero UB.

**Directions.**

- *Door shape — decided.* Stage the sources' erased forms as one slice of a run family,
  re-anchor it through the single existing retype alongside the destination operand, and let
  the relocate hook see every source at the shared brand. The pairwise door stays for genuine
  1:1 sites (`relocate_seam`, the `catch` and tag arms), so this is an addition, not a
  replacement. Branch `spike/nary-transfer-door` (`14e384f2`) carries a working instance of
  this shape and the measurements above; treat it as a reference, not a base.
- *Caller currency — decided.* The public door takes `&[Self]`; a crate-internal engine
  generic over `impl ExactSizeIterator<Item = &Self>` sits behind it, and `alloc_with` keeps
  its `&[&Self]` signature by feeding the engine `deps.iter().copied()` — its builtin callers
  hold individual borrows (`record_projection`'s lhs comes from `arg_carrier`), so a `&[Self]`
  ripple there would force the cloning the acceptance criteria forbid elsewhere.
- *Alignment enforcement — decided.* The pairing: the relocate hook returns the product plus a
  per-source cells run (a dedicated family parameter), and the door itself zips that run with
  the source envelopes to call the predicate as `(source, its cell, region)` — no caller-facing
  index. The residual staging-order contract on the relocate hook is stated on the door and
  checked by a debug assert on the run lengths.
- *Small-N cost — decided.* Shrink; no length dispatch. `smallvec` staging and pin gather,
  the borrowed pin slice itself serving as the staging `Witness` in place of a union
  allocation, and the per-source retention bundles folded into a single union-retained walk
  that builds the composed antichain directly — measured against the pairwise path's
  allocation count at N=2 by a counting-allocator test.
- *Holder coverage — decided.* Per-cell: the relocate closure zips the staged run with the
  source-envelope slice it captures, so each cell's rebuild proves its holder rule against its
  own coverage — the pairwise contract preserved exactly, no union widening. The zip rides the
  same stated staging-order contract as the alignment pairing.

## Dependencies

**Requires:** none.

**Unblocks:** none.
