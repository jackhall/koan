# `Sealed`: a single access verb

With the scope reads folded, the allocator confined, and the scope pointers collapsed, delete the
now-callerless borrow-bounded `attach` and the `reattach_ref_with` witness-borrow read path, leaving
`Sealed` / `SealedExtern` with one access verb.

**Problem.** The FrameStorage restructure landed a scope-specialized
[`SealedExtern<ScopeRefFamily>::attach`](../../src/machine/core/arena.rs) — a borrow-bounded
`&'w Scope<'b>` re-anchor (routing [`reattach_ref_with`](../../src/witnessed.rs)) that the frame's
child-scope readers use to return a re-anchored scope up the dispatcher stack, the shape the
keystone's `for<'b>` `open` forbids by construction. It is the borrow-bounded accessor that lets a
re-anchored reference ride up-stack — a second access verb beside [`open`](../../src/witnessed.rs).
(Its self-witnessed twin `Sealed::read` is already gone, deleted by the value-read migration.)

**Acceptance criteria.**

- `Sealed` / `SealedExtern` expose a single access verb — `open` (plus its consuming
  externally-witnessed twin): `attach`, the externally-witnessed witness-borrow read path, and
  `reattach_ref_with` are deleted, and no call site references any of them.
- Any reader that provably cannot nest under `open` is surfaced and documented as the lone exception,
  not silently retained; no speculative generic borrow-bounded `attach` is added.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Open-only is the destination — decided.* A single access verb is the substrate's target surface;
  this item is the cleanup that confirms no consumer still needs the borrow-bounded one once the
  scope channel folds into the step `open`.
- *No speculative generic `attach` — decided.* The shipped `attach` is scope-specialized; rather than
  generalizing it to a `Sealed<T>` verb on spec, every site prefers `open` + copy-out, and the survey
  for an un-nestable non-scope reference happens here. A generic borrow-bounded `attach` is added
  *only* if such a site is found, surfaced with why it cannot fold — never as a default escape hatch.
- *Gated on a clean residue — decided.* If a consumption path proves un-invertible and still holds an
  `attach`, that is surfaced here rather than silently retained; the residue is closed before
  deletion, not worked around.

## Dependencies

**Requires:**

- [Collapse the scope-pointer erasure into the substrate](scope-pointer-collapse.md) — clears
  `ErasedScopePtr`'s use of `reattach_ref_with`.
- [Confine `Region::alloc` to a brand](region-alloc-brand-confined.md) — clears `Region::alloc`'s use
  of `reattach_ref_with`.

**Unblocks:** none.
