# Shape errors name their source location

**Problem.** A [`ShapeError`](../../src/scope/shape.rs) locates itself by a
`Position`, the index of a statement in the body being built, and renders it as
`statement N` or `a parameter`: it never names a line, nor which body the
statement is in. `EagerCycle` names its members and no location at all. The
[operator-run rewrite](../../src/scope/shape/build/rewrite.rs) builds its nodes
from `Spanned::bare` parts, so the statements of the block a pairwise run
synthesizes carry no span, and an error in one names a statement of a block the
program never wrote.

**Acceptance criteria.**

- A shape error names the source location it was found at, `path:line:col`.
- Every statement the operator-run rewrite builds carries the span of the
  source it was built from, so an error in a rewritten statement points into the
  source rather than at a statement number of the built body.

## Dependencies

**Requires:** none — foundation.
