# Declaration identity is the installing node

**Problem.** The two same-declaration checks — `finalize_nominal_member` in
[`resolver.rs`](../../src/machine/model/types/resolver.rs) and `recover_union` in
[`union.rs`](../../src/builtins/union.rs) — ask "is this the binding my own declaration
installed?" and answer it by comparing the `BindingIndex` stored beside the committed
type against their own `bind_index`. The index is a lexical position, not an identity:
it discriminates only as finely as the chain that minted it.
[`enter_block`](../../src/machine/execute/runtime/submit.rs) mints `(scope_id, i + 1)`
per statement, so declarations reached that way always differ. A submission that
inherits no ambient payload falls back to `LexicalFrame::detached`, whose index is `0`
for every statement in the scope — distinct declarations of one name then compare equal
and take the idempotent short-circuit where a `Rebind` belongs.

The datum that answers the question exactly is already minted: the binder's install
stamps the producing `NodeId` into `placeholders`, which stores
`(NodeId, BindingIndex, BindKind)`. A parallel finalize is that same scheduler slot
re-entering. Only `types` narrows its entry to `(&'a KType, BindingIndex)`, forcing the
finalize checks to reconstruct declaration identity from position instead of reading the
node that installed the binding.

**Acceptance criteria.**

- A committed `types` entry records the `NodeId` that installed it, and both
  same-declaration checks decide on that node rather than on a lexical position.
- Two declarations of one name in a single scope raise `Rebind` regardless of the
  lexical chain the submission carries, detached chains included.
- A parallel finalize of one declaration still short-circuits idempotently, pinned by
  `finalize_union_seals_then_is_idempotent` and
  `block_member_seals_shared_set_then_short_circuits_before_rebind`.

**Directions.**

- *Widen the `types` entry rather than adding a side map — decided.* `placeholders`
  already pairs a `NodeId` with a `BindingIndex`; a parallel map keyed by name would
  duplicate the write paths that keep the two in step.
- *`BindingIndex` stays in the entry alongside the node — decided.* The visibility
  predicate `idx < cutoff` reads it, so the node identifies the declaration and the
  index continues to answer the forward-reference question.
- *Whether the detached-chain fallback survives — open.* Minting real per-statement
  indices in `dispatch_in_scope` would close the same gap at the submission end instead.
  It is not an alternative to the node-identity change so much as a separate question
  about whether a submission may carry an index that names no statement. Recommended:
  decide it once the entry carries the node, when the index has one job left.

## Dependencies

The gap is latent rather than live: the only production entry into a top-level block,
[`interpret`](../../src/machine/execute/runtime/interpret.rs), goes through
`enter_block`, so the detached-chain fallback is reached from tests and
`builtins/test_support.rs` alone.

**Requires:** none — the binder-install path already carries every datum this needs.

**Unblocks:** none.
