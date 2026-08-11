# Region debug audits

Debug-mode observability for the over-pinning direction, which every
existing audit passes silently.

**Problem.** Every residence audit catches under-pinning only; over-pinning
has no observability at all. Two concrete shapes. Cross-region pin cycles:
reach sets hold `Rc<FrameStorage>` members
([region.rs](../../workgraph/src/witnessed/region.rs)); home-omission breaks
the self-cycle by construction, but a mutual pin — region A's set retaining
frame B while region B's set retains frame A — is expressible in safe code,
defeats the refcount-driven region free, and nothing detects it short of the
Miri leak slate. Reach over-approximation: folding a dep a value did not
actually borrow from keeps that dep's region alive as long as the carrier
lives; the `Scalar` bound on the scalar doors (`alloc_scalar` /
`alloc_scalar_witnessed`, [arena.rs](../../src/machine/core/arena.rs))
counters the known case, and the folded sink (`alloc_object_folded`) runs no
audit in either direction — the placement capability discharges the store at
compile time, trusting the fold's declared coverage.

**Acceptance criteria.**

- A debug-mode cycle detector walks the region-set graph and reports mutual
  pins; a test constructing a mutual pin observes the report.
- A debug-mode reach-tightness report compares the regions a carrier pins
  against the regions its value actually references and flags over-folds; a
  test with a deliberately over-approximated fold observes the flag.
- Both audits are compile-gated (debug / test feature) with no release-build
  cost.

**Directions.**

- *Cycle handling — decided.* Detect and report in debug builds, online at the
  retention fold that closes the ring (a detached mutual pin is unreachable
  from any root afterwards, so an on-demand walk cannot find it). A structural
  rule making mutual pins unrepresentable is a separate design if the detector
  shows cycles arise in practice. The detector lives in `workgraph` with a
  small report surface under `cfg(debug_assertions)`; the tightness report
  lives in Koan behind a `region-audit` feature, instrumenting the
  `alloc_carried_with` chokepoint only.
- *Tightness ground truth — decided.* Instrument the witness composition to
  record which operands contributed. The alternative — walking a stored value's
  borrows against a recorded address table — is not available: a region keeps no
  address table, and residence is answered by the value's own field or by the
  door that placed it.

## Dependencies

Bind-side reach over-approximation closes by construction — the fused bind doors seal the value to a
description the library minted for it
([design/witness-hosting.md § Scope and bindings](../../design/witness-hosting.md)); this item's
tightness report covers the fold side and the cycle case.

**Requires:** none — additive diagnostics.

**Unblocks:** none tracked.
