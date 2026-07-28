# Residence-audit retirement

**Problem.** The runtime residence audit — the
[`Residence`](../../src/machine/core/arena/residence.rs) ownership predicate
and the evidence-tier move-in doors layered on it — vets every composite
move-in at runtime against reach evidence the scopes mint and thread by hand.
The reaching tier re-checks what the fold-brand construction door already
compile-enforces:
[`FoldingBrand::alloc_object_folded`](../../src/machine/core/arena.rs) stores a
value built from a fold's declared operands with no runtime audit — an
ambient-lifetime capture is a compile error at the signature — and the copy
path ([`rebuild_delivered_substrate`](../../src/machine/core/scope/reach.rs))
already routes through it. Only the pin/adopt-reaching path still runs the walk,
so its residence audit reads as enforcement rather than the stopgap it is.

**Acceptance criteria.**

- The reaching/evidence-tier residence machinery is deleted, replaced by
  fold-brand construction doors (`transfer_into_placing` +
  `FoldingBrand::alloc_object_folded`) that compile-enforce residence: the
  pin/adopt-reaching stores (`store_object_adopted`/`store_object_pinned`),
  their shared audit (`store_value_reaching`, `store_projection_reaching`), the
  module reaching stores (`store_module_object`, `store_transparent_view`), the
  `Residence` struct's reach/ambient arms, `resident_in_delivered` /
  `resident_in_visiting`, and `ResidenceEvidence::reaching_ambient`.
- The subsumed by-construction re-checks are deleted: the KFunction-wrap in
  [`builtins.rs`](../../src/builtins.rs) and the rebuild re-box in
  [`scope/reach.rs`](../../src/machine/core/scope/reach.rs) — the value is
  already residence-established by the prior alloc / the fold placement.
- Every retained runtime residence-audit site the design doc's end state keeps
  is documented as a redundant backstop in
  [design/witness-hosting.md](../../design/witness-hosting.md): the dest-only
  `resident_in` splice-free gates (`alloc_object_witnessed_checked` and the
  resolved-literal/quote path) and the primitive `ptr::eq` guards
  (`alloc_function`, `alloc_scope`, `alloc_module`) — with a note stating that
  [drop-free region death](drop-free-region-death.md) removes the splice-free
  gate. The retained `alloc_object_checked_stored` home-borrow-bit walk is left
  unmentioned: home is an ordinary reach member
  ([witness-hosting.md](../../design/witness-hosting.md)), which subsumes it.
- The Miri audit slate is green after the retirements.

## Dependencies

**Requires:**


**Unblocks:**

- [Drop-free region death](drop-free-region-death.md) — the reaching tier is
  retired here via the fold-brand doors; that item deletes the residual
  dest-only `resident_in` walk once the expression-part families are
  `Drop`-free, and migrates the families to the untyped bump arena.
