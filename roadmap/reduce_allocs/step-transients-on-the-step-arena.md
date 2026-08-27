# Step transients on the step arena

**Problem.** Four small vectors on the per-step path are built on the global allocator, and
each costs a heap allocation per step for a handful of elements that are dead before the step
ends. A dhat difference of `audit/shapes/wide_n{10,100}.koan` puts them together at 21
allocations per step.

- [`submit_expression`](../../src/machine/execute/decide/submit.rs) collects the claim edges a
  binder stamps into a `Vec<EdgeId>` built from empty, one per installing statement. The
  vector is handed to the slot and dropped inside the same step.
- [`split_chain_parts`](../../src/machine/execute/decide/operator_chain.rs) splits an operator
  chain's parts into two `Vec`s, and `chain_operator_symbols` beside it collects the chain's
  keywords into a third — all three read and dropped inside the dispatch that built them.

The arena for exactly this is already installed: `ctx.scratch()` is a bump handle reset at
every drain pop, and the wiring door takes one
([`install_deps_in`](../../workgraph/src/scheduler.rs) hosts its verdict list on the caller's
allocator for the same reason). What these four sites lack is not a facility but the use of
one.

One neighbouring buffer is *not* a step transient and needs a different answer.
[`SlotFrame::own_edges`](../../src/machine/execute/nodes.rs) extends the slot anchor's
`RefCell<Vec<EdgeId>>`, which lives until the slot terminalizes and hands its edges to the
incarnation that replaces it — so it outlives the pop and cannot go on the arena that pop
resets. It is 17 allocations per step, and every one of them is a vector of a few `EdgeId`s.

**Acceptance criteria.**

- The claim-edge list in `submit_expression`, both halves of `split_chain_parts`, and
  `chain_operator_symbols`' keyword list are hosted on the step's scratch arena, and a dhat
  difference of the wide pair attributes no allocation to any of them.
- A slot anchor's owned-edge list allocates nothing for a statement that stamps a typical
  number of claims, without moving the list onto an arena that outlives it.
- The `wide_step` term in [`observe/alloc.txt`](../../observe/alloc.txt) falls by the
  attributed share, and `tests/allocation_baseline.rs` holds the new figure.

**Directions.**

- *Where the four transients live — decided.* `ctx.scratch()`, per
  [design/execution/scheduler.md § The dispatcher / scheduler boundary](../../design/execution/scheduler.md#the-dispatcher--scheduler-boundary):
  anything the step itself consumes is built through the drain's per-pop bump.
- *How the anchor's owned-edge list avoids its allocation — open.* An inline-capacity vector
  sized to the common claim count, which keeps the list where it is and costs the anchor a
  fixed inline array; or the cart's own region, which the slot already outlives to the same
  moment. Recommended: inline capacity, since the count is small and bounded by the binder
  form rather than by the program.
- *Whether the split in `split_chain_parts` is needed at all — open.* The two halves are the
  even and odd positions of one part list; a fold that indexes the original would build
  neither vector.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none tracked yet.
