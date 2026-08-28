# Fresh-cart block freeze

**Problem.** A block's statements are frozen from raw AST into working form
against the region its caller names
([`block_tail`](../../src/machine/core/kfunction/block_tail.rs)), and the
frozen run travels out on [`Action::tail`](../../src/machine/core/kfunction/action.rs)
at the caller's own lifetime. A block whose cart is a frame the tail installs
has no such region to name yet — the fresh frame's region is younger than
that lifetime — so `CLOSE OVER` freezes its body into the eternal region
instead ([close_over.rs](../../src/builtins/close_over.rs)). That region is
released only when the run ends, so each *evaluation* of a `CLOSE OVER` adds
one statement-run copy to the run-root arena for the rest of the run: a form
re-entered many times, such as one carried through a recursion, grows that
arena without bound.

**Acceptance criteria.**

- A block whose frame the tail installs freezes its statements into that
  frame's own region, and the copy is released when the region dies.
- Evaluating one `CLOSE OVER` `n` times leaves the run-root arena's
  high-water mark flat in `n`, observed through the allocation substrate.
- Every other `block_tail` caller keeps the home it has today: an overlay
  block's statements stay in the cart-ancestor region, and a non-statement
  body still freezes to itself.

**Directions.**

- *Where the freeze happens — settled (2026-08-28): the drain freezes it once
  the frame is installed.* The body crosses the install as raw AST on a new
  raw-body `Action` variant, and the reinstalled step — running with the
  fresh cart as its own scope — freezes the working run at that brand, so
  the copies live in the cart's region and die with it. This is the shape
  the FN-call path already uses (`body_continue` in
  [decide/exec.rs](../../src/machine/execute/decide/exec.rs) carries the
  callee's body raw and freezes at the installed cart's brand), reached
  through `Replacement::fresh_child` rather than `fresh_tail` so the
  caller's cart does not retire. The alternative — freezing inside the
  install and storing the run self-referentially in the frame — was
  rejected: it needs new frame storage plus a branded read-back door to
  reach the same end state.

## Dependencies

**Requires:** none — a refinement of shipped block machinery.

**Unblocks:** none.
