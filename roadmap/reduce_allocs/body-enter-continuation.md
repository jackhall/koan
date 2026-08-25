# Body-enter continuation

**Problem.** Entering a user function body heap-allocates its continuation twice over:
`body_continue` in
[src/machine/execute/decide/exec.rs](../../src/machine/execute/decide/exec.rs) captures
an `Rc<CallFrame>`, an owned `Vec` of leading statements, the tail expression, and the
tail contract into a boxed resume — even though the same `Rc` already rides the
outcome's `FramePlacement::FreshTail` and becomes the slot's own cart at install — and
the leading-statements tail finish in
[src/machine/execute/decide.rs](../../src/machine/execute/decide.rs) boxes again,
holding a second block-frame handle. The end state is the co-location rule in
[design/execution/continuations.md](../../design/execution/continuations.md): the
body-enter continuation bumps into the fresh cart's own region.

**Acceptance criteria.**

- `body_continue`'s continuation erases on the bumped tier, in the fresh cart's region:
  the cart is re-derived from the slot's anchor at wake, never captured, and the
  leading statements ride as a region slice, not an owned `Vec`.
- The fresh-cart bump door this builds also moves the `FreshChild`/`FreshTail` tail
  decides in `tail_continue` onto the bumped tier — they ship Boxed from
  [outcome-obligation-boxing](outcome-obligation-boxing.md), whose bumped host is the
  current frame only.
- The leading-statements tail finish's capture set slims to `Copy` data plus its
  load-bearing block-frame `Rc` (nothing else keeps the block frame alive across that
  park), which keeps that finish on the Boxed tier.
- The `user_fn_params1` / `user_fn_params8` terms in
  [`observe/alloc/terms.txt`](../../observe/alloc/terms.txt) drop by the removed boxes,
  and the affected bounds in `tests/allocation_baseline.rs` are re-measured.

## Dependencies

**Requires:**

- [Outcome and obligation boxing](outcome-obligation-boxing.md)

**Unblocks:** none.
