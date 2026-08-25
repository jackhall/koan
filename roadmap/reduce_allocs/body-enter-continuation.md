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
  decides in `tail_continue` onto the bumped tier: `decide_tail`'s `host` is `None` for
  both placements, since
  [`erase_bumped`](../../src/machine/execute/outcome.rs) hosts only in a brand the
  deciding step already holds.
- The leading-statements tail finish's capture set slims to `Copy` data plus its
  load-bearing block-frame `Rc` (nothing else keeps the block frame alive across that
  park), which keeps that finish on the Boxed tier.
- The `user_fn_params1` / `user_fn_params8` terms in
  [`observe/alloc/terms.txt`](../../observe/alloc/terms.txt) drop by the removed boxes,
  and the affected bounds in `tests/allocation_baseline.rs` are re-measured.

**Directions.**

- **Where the bumped host brand comes from — decided: a coupled replacement currency.**
  `Outcome::Continue` carries a `Replacement` — the frame placement and the work as one
  private-field value whose constructors are the only doors. `inherit(work)` keeps the
  slot's cart; `fresh_tail` / `fresh_child` take the `Rc<CallFrame>` the placement
  installs and hand their build closure a host brand minted off that same frame, so
  pairing a fresh placement with work hosted in a sibling cart's region is
  unrepresentable at the construction sites — the co-location rule as a constructor
  operand rather than prose. Entailed: a `Continue`'s work rides **pre-erased**
  (`erase_to_static::<ContinuationFamily>` inside the constructor, where the host
  borrow must end before the frame moves into the placement) — the same erased currency
  `StepVerdict::Replace` already carries, minted one hop earlier; the work is stored,
  never run, until the drain seals it against the slot's effective anchor exactly as
  today.

## Dependencies

**Requires:** none.

**Unblocks:** none.
