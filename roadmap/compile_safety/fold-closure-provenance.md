# Fold-closure capture provenance

**Problem.** A closure under a reach-folding combinator (`transfer_into` /
`merge_pinned` / `map_pinned` in
[workgraph/src/witnessed.rs](../../workgraph/src/witnessed.rs),
`alloc_carried_with` in [arena.rs](../../src/machine/core/arena.rs)) is
covered only for values built from the closure's own operands — but a `move`
closure can capture any outside reference and embed it in the built value.
Everything unifies at the brand lifetime and compiles clean; the composed
witness never names the smuggled borrow. The fold-capability type confines
*where* folded placements happen, not *what* the closure captures. Fourteen
sites ride the fold surface today: one `map_pinned`
([dispatch/literal.rs](../../src/machine/execute/dispatch/literal.rs)), one
direct `alloc_carried_with` ([attr.rs](../../src/builtins/attr.rs)), and
twelve `alloc_type_with` / `alloc_object_with` calls across the builtins and
dispatch.

**Acceptance criteria.**

- A fold closure cannot embed a reference captured from outside its
  operands: the fold surface either rejects capturing closures at compile
  time or receives every input the closure builds from as a declared
  operand. A `compile_fail` test pins the capture-smuggle rejection.
- The current fold call sites compile through the tightened surface with
  their built values unchanged.
- The full test suite and the Miri audit slate are green across the change.

**Directions.**

- *Discipline mechanism — open.* (a) Non-capturing bounds — take `fn`
  pointers (or a zero-sized-closure bound) so captures are rejected
  wholesale, with all inputs passed as operands; (b) restructure the fold
  doors so the closure receives an operand view bundle and nothing else is
  nameable at the brand lifetime; (c) accept capture but audit the built
  value's borrows at fold (runtime, weakest). Recommended: spike (a) first —
  it is the only shape that produces a compile error.

## Dependencies

Soft ordering: [Witness-derived binding](witness-derived-binding.md) may
shrink what fold closures build, so it lands first when both are in flight.

**Requires:**

- [Sparse-carrier correlation in parameterized-type constructors](sparse-carrier-correlation.md)
  — the constructor sites must correlate views to args before their captures
  can move inside the brand.

**Unblocks:**

- [Eliminating the workgraph escape hatch](escape-hatch-elimination.md) —
  the folded-tier retype is the first tier of deleting the audited move-in.
