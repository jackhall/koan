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
lives; the scalar gates in `alloc_type_with` / `alloc_object_with`
([arena.rs](../../src/machine/core/arena.rs)) counter the known cases, and
the wrong direction of the `borrows_into_home` bit (`true` when false) is
the same hole — a retiring tail-call frame riding the fresh frame's
bindings.

**Acceptance criteria.**

- A debug-mode cycle detector walks the region-set graph and reports mutual
  pins; a test constructing a mutual pin observes the report.
- A debug-mode reach-tightness report compares the regions a carrier pins
  against the regions its value actually references and flags over-folds; a
  test with a deliberately over-approximated fold observes the flag.
- Both audits are compile-gated (debug / test feature) with no release-build
  cost.

**Directions.**

- *Cycle handling — open.* (a) Detect and report in debug builds; (b) a
  structural rule making mutual pins unrepresentable. Recommended: (a) now —
  (b) is a separate design if the detector shows cycles arise in practice.
- *Tightness ground truth — open.* (a) Walk the stored value's borrows via
  the recorded side-tables; (b) instrument the witness composition to record
  which operands contributed. Recommended: (b) — the side-tables are
  deliberately partial.

## Dependencies

Bind-side reach over-approximation closes by construction in
[Witness-derived binding](../compile_safety/witness-derived-binding.md); this item's tightness
report covers the fold side and the cycle case.

**Requires:** none — additive diagnostics.

**Unblocks:** none tracked.
