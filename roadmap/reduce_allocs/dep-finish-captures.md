# Dep-finish captures

**Problem.** The engine's dep-finish sites each box their finish closure per park and
capture owned `Vec`s that could be region slices: the eager-subs rebuild
([keyworded.rs](../../src/machine/execute/decide/keyworded.rs)), the pairwise operator
fold ([operator_chain.rs](../../src/machine/execute/decide/operator_chain.rs)), the
applied-type arguments
([apply_callable.rs](../../src/machine/execute/decide/apply_callable.rs)), the
deferred-head classify
([head_deferred.rs](../../src/machine/execute/decide/head_deferred.rs)), the field-list
rewalk ([field_list.rs](../../src/machine/execute/decide/field_list.rs)), the Forward
obligation checker ([harness.rs](../../src/machine/execute/harness.rs)), the construct
combine ([constructors.rs](../../src/machine/execute/decide/constructors.rs)), the
aggregate literal ([literal.rs](../../src/machine/execute/decide/literal.rs)), and the
sigiled type leaf ([single_poll.rs](../../src/machine/execute/decide/single_poll.rs)).
Two of them nest a second boxed currency inside the finish: the field-list
`BrandCompose` and the aggregate `AggAssemble`. The end state is
[design/execution/continuations.md](../../design/execution/continuations.md): each
finish is a `Copy` closure on the bumped tier, its list captures region slices, its
nested currencies generic compositions folded in before the one erasure.

**Acceptance criteria.**

- Every site above erases on the bumped tier: no `Box::new` at the site, list captures
  (staged part indices, operator runs, operand spans, threaded symbols, aggregate rows,
  argument names) as bump slices in the closure's region.
- `BrandCompose` and `AggAssemble` no longer exist as boxed currencies — each composes
  generically into its finish before the erasure.
- The field-list lexical-chain `Rc` re-derives from the ambient node payload at wake
  (to confirm at planning; if it is not re-derivable, that finish stays Boxed and this
  criterion is amended to record why).
- The `dispatch` and `tagged_construct` terms in
  [`observe/alloc/terms.txt`](../../observe/alloc/terms.txt) drop by the removed boxes,
  and the affected bounds in `tests/allocation_baseline.rs` are re-measured.

## Dependencies

**Requires:**

- [Outcome and obligation boxing](outcome-obligation-boxing.md)

**Unblocks:** none.
