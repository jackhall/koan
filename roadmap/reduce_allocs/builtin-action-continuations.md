# Builtin action continuations

**Problem.** The [`Action`](../../src/machine/core/kfunction/action.rs) currency's
builtin surface allocates on the heap per steady tail-loop step. The dhat attribution
(2026-08-25, per iteration) names the sites. Every MATCH step buys the `BlockSeed` box
`arm_tail` mints for a seed that runs synchronously inside
[`block_tail`](../../src/machine/core/kfunction/block_tail.rs), the statement-split
`Vec` under `block_tail`, and — in
[`find_branch_body_by_type`](../../src/builtins/branch_walk.rs)'s selection walk — an
eager `head_label` `String` plus `exact_arms` `Vec` growth. A multi-statement body adds
the leading-statements finish box (`finish_terminal_boxed` in `run_action`'s `Tail`
lowering), the leading-statement collects in `body_continue` / `body_statement_refs`,
and three `Vec` allocations per effectful step on the `Action::effects` channel
(`with_effect` → `with_effects` → `deposit_effects`). A TRY-carrying step adds the
`CatchContinue` box and its boxed erasure in `run_action`'s `Catch` arm — even though
TRY's finish captures are all `Copy`. The `AwaitDeps` / `AwaitBlock` finishes
(`AwaitContinue`) fire on no steady-state path and stay on the Boxed tier of
[design/execution/continuations.md](../../design/execution/continuations.md).

**Directions.**

- *Seed — decided.* `block_tail` goes generic over `impl FnOnce`; the boxed `BlockSeed`
  alias is deleted.
- *Statement split — decided.* `ActionKind::Tail`'s `leading` and
  `BlockRequest::Body`'s `statements` become host-region slices, and a body that is not
  a statement block takes the `Single` lowering without touching the splitter.
- *Effects — decided.* `Action` loses its `effects` field; a builtin body or finish
  writes each `WriteOp` through a `BodyCtx` / `FinishCtx` door into the harness-owned
  per-step sink, whose buffer is reused across steps. Program order and the
  all-or-nothing apply are unchanged.
- *Catch tier — decided.* The `Catch` finish currency goes two-tier like
  `ContinuationCall`: a `Copy` finish erases onto the bumped tier at the builtin
  construction site, the boxed twin remains for owning captures. TRY moves; CATCH moves
  if its captures audit `Copy`.
- *Leading-statements finish — open.* Its box exists to own the block-frame `Rc`.
  Either the frame is re-derivable at wake (the leading statements' own slots hold it
  across the park — to confirm), putting the finish on the bumped tier, or the capture
  is genuinely load-bearing and the box is documented as the accepted Boxed-tier
  remainder.
- *Selection walk — decided.* `find_branch_body_by_type`'s arm lists become step-scratch
  transients and arm labels render only on the diagnostic path.

**Acceptance criteria.**

- A steady-state step of the three loop shapes allocates nothing on the `Action`
  surface: a dhat re-profile of `audit/shapes/tail_loop_steps100.koan` and the two
  shapes below attributes no per-step term to `arm_tail`, `block_tail`,
  `find_branch_body_by_type`, the effects channel, or `run_action`'s `Tail` / `Catch`
  lowering — except, for the leading-statements finish only, a per-step box that the
  spike proved load-bearing and
  [design/execution/continuations.md](../../design/execution/continuations.md)
  documents as such.
- `audit/shapes/leading_loop_steps{10,100}.koan` (a tail loop whose FN body carries a
  leading `LET`) and `audit/shapes/try_loop_steps{10,100}.koan` (a tail loop with a TRY
  per iteration) are in the sweep, each with a term in
  [`observe/alloc/terms.txt`](../../observe/alloc/terms.txt) and an absolute bound in
  `tests/allocation_baseline.rs`.
- The `step` term drops by the removed share and the tail-loop bound in
  `tests/allocation_baseline.rs` is re-measured.

## Dependencies

**Requires:** none.

**Unblocks:** none.
