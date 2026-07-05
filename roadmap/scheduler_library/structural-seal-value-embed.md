# Structural `seal_value` embed

**Problem.** `Scope::seal_value` (`src/machine/core/scope.rs:844`) still
takes `embedded: Option<&Sealed<…>>`: a caller sealing a projected value
passes the source carrier to fold its foreign reach, or asserts a
region-pure source by passing `None`. Its callers — the field reads in
`src/builtins/attr.rs` (`access_field`) and FROM's projection in
`src/builtins/record_projection.rs` — thread the `Option` from
`FinishCtx::arg_carrier`, where `None` stands for "no foreign reach". The
step construction context makes purity structural at its born-pure sites
(`ctx.alloc` = own region only, `ctx.alloc_with` = deps folded — see
[design/scheduler-library.md](../../design/scheduler-library.md));
`seal_value` is the last seal surface where a value's reach is an
asserted-or-absent operand rather than named by construction.

**Acceptance criteria.**

- The at-will seal surface carries no `Option` reach operand: a region-pure
  seal and an embedded-reach seal are distinct constructions, each naming
  its reach structurally in the call shape.
- The `attr.rs` field reads and `record_projection.rs`'s projection build
  their result carriers through the structural surface; no call site passes
  `None` to assert purity.
- Sealed reach-sets are unchanged for both pure and embedded sources —
  existing attr / FROM tests green.

**Directions.**

- *Surface shape — open.* (a) Split methods — a pure-only `seal_value` plus
  an embedding variant taking `&Sealed`; (b) an alloc-combinator shape
  mirroring `ctx.alloc` / `ctx.alloc_with` on the at-will surface, so the
  allocation and the reach fold are one construction. Recommended: (b) —
  "named by construction" is the property the step-context precedent
  establishes, and (a) keeps the fold a separate assertable step.
- *Where the pure/embedded branch lands — open.* `FinishCtx::arg_carrier`
  legitimately returns `None` (no entry means no foreign reach), so
  retiring `seal_value`'s `Option` must not merely relocate the assertion
  into an `if let` at each caller. Either the pure arm is a genuinely
  distinct construction at these sites, or folding an empty foreign reach
  is a no-op and a single embedding construction over the source carrier
  retires the branch entirely.

## Dependencies

Touches the same at-will allocation/seal surface
[regions-wholesale](regions-wholesale.md) re-homes; the two can land in
either order, but whichever lands second inherits the first's surface
shape.

**Requires:** none — independent of the boundary moves.

**Unblocks:** none tracked.
