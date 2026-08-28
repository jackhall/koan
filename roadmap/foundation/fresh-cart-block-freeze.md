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

- *Where the freeze happens — open.* The freeze currently precedes the frame
  install, which is what forces the region choice. Two shapes reach the
  fresh cart: freeze inside the install, so the working run is built at the
  frame's own brand and `Action::Tail` carries it at that region's lifetime;
  or carry the body unfrozen and let the drain freeze it once the frame is
  installed. Both change `Action::tail`'s shape, so the choice is about
  which lifetime the tail's statement run is quantified over.

## Dependencies

**Requires:** none — a refinement of shipped block machinery.

**Unblocks:** none.
