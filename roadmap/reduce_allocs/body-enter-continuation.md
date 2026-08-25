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

- **Where the bumped host brand comes from — open.** `erase_bumped` takes any
  [`RegionBrand`](../../src/machine/core/arena.rs) live at the step brand, so the
  co-location rule — the host is the region of the frame the work installs under — holds
  by call-site discipline rather than by the type. Every site obeys it structurally
  today (a park and an `Inherit` replace both keep the slot's cart, which is why
  `tail_continue` reads the placement), but a sibling cart's brand would compile and
  would dangle the moment that sibling's frame died first. This item already has to mint
  a brand for a cart the slot does not yet stand in, so the choice is where that brand is
  sourced from. Options: thread the host as a construction operand carrying the frame it
  installs under, the way a yoked dest brand rides the resident type carrier, so a
  mismatched region is a type error; or keep the ambient-brand door and state the rule in
  prose. *Recommended:* the operand — the fresh-cart door has to name its frame anyway,
  so the enforcement falls out of the work rather than sitting beside it.

## Dependencies

**Requires:** none.

**Unblocks:** none.
