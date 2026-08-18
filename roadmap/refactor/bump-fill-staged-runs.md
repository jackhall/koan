# Runs staged on the heap before a bump copy

**Problem.** [`BumpAllocator::slice_from_iter`](../../workgraph/src/witnessed/bump.rs) fills a
region run straight from an exact-length iterator, so a caller that computes its elements pays one
copy into the bytes the value keeps. The dispatch path takes it; every other run-building site in
the tree still collects an owned `Vec` and hands it to `slice`, paying a heap allocation and a
second copy for a run the region was always going to hold.

The sites, each a `collect()` (or a push loop) whose only consumer is the `slice` on the next line:

- **A parsed node's parts run.** `KExpression::build`
  ([src/machine/model/ast.rs](../../src/machine/model/ast.rs)) takes
  `Vec<Spanned<ExpressionPart>>` by value and opens with `brand.allocator().slice(&parts)` — the
  AST twin of the `WorkingExpression` door, which now takes a borrowed run or an iterator. Its
  callers build the `Vec` for no other purpose.
- **Literal runs, at parse.** `ExpressionPart::{List,Dict,Record}Literal` are built from a staged
  run in `peel` ([src/parse/expression_tree.rs](../../src/parse/expression_tree.rs), three sites)
  and in the frame reducer ([src/parse/frame.rs](../../src/parse/frame.rs), three more).
- **Signature element runs.** Four sites in
  [src/machine/model/types/signature.rs](../../src/machine/model/types/signature.rs), including
  `ExpressionSignature::mint`, which every function definition runs.
- **Declaration-window runs.** Six sites in
  [src/machine/model/types/declaration_window.rs](../../src/machine/model/types/declaration_window.rs).
  One of them, the `fills` initializer, is `slice(&vec![None; data.members.len()])` — a heap
  allocation and a copy to store a run of `None`.
- **Bucket keys, group members, field names.**
  [src/machine/model/binder.rs](../../src/machine/model/binder.rs),
  [src/machine/model/operators.rs](../../src/machine/model/operators.rs),
  [src/machine/model/values/kobject.rs](../../src/machine/model/values/kobject.rs).

Two further sites are *not* in this set and should stay as they are: the relocation doors in
[src/machine/execute/decide/literal.rs](../../src/machine/execute/decide/literal.rs) and
[src/machine/execute/decide/constructors.rs](../../src/machine/execute/decide/constructors.rs)
hand the bumped slice back twice — as the product's run and as the per-source cells the door pairs
with its envelopes — so the staged run is the fold's own working state, not a copy buffer.

None of these sits on the per-step or per-dispatch path, so none moves the scaling terms
[`audit/README.md`](../../audit/README.md) records. What they cost is per parse, per definition
and per declaration.

**Acceptance criteria.**

- `KExpression`'s construction doors take `&[Spanned<ExpressionPart<'a>>]` or an exact-length
  iterator, and no call site builds an owned run solely to hand it to one.
- Every site listed above builds into the region bump directly; `grep` for
  `allocator().slice(&` over `src/` matches only stack arrays, runs a caller already holds for
  another reason, and the two relocation doors named above.
- The `fills` initializer reserves its run without materializing a `Vec` of `None` first.
- An empty program's recorded allocation count drops, and the per-step and per-dispatch terms in
  [`audit/README.md`](../../audit/README.md) are unchanged — these sites are one-shot, so a moved
  scaling term means something else moved with them.

**Directions.**

- *Verb — decided.* `BumpAllocator::slice_from_iter`; it shipped with the dispatch-path work and
  needs no extension.
- *`KExpression`'s door set — open.* Mirror `WorkingExpression`'s split (`build` over a borrowed
  run plus a `build_from_iter` peer over an iterator), versus a single iterator-only door with
  fixed-length callers passing `<[_; N]>::into_iter`. Recommended: mirror the split, so the two
  expression families read the same way at their call sites.
- *Fallible fills — open.* A site whose per-element step can fail cannot fill an
  `ExactSizeIterator` directly; the dispatch-path work handled its one such case
  (`reconstruct_positional`) by hoisting the check ahead of the fill. Whether the remaining sites
  have any, and whether hoisting generalizes or some should keep staging, is unsurveyed.

## Dependencies

**Requires:** none — the verb it spends already shipped.

**Unblocks:** none.
