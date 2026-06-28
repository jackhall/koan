# `Sealed`: a single access verb

Delete the transitional `attach` and the externally-witnessed witness-borrow read path once every
consumer is on `open`, leaving `Sealed` with one access verb.

**Problem.** The shipped FrameStorage restructure landed a scope-specialized
`SealedExtern<ScopeRefFamily>::attach` (a borrow-bounded `&'w Scope<'b>` re-anchor — see
[`arena.rs`](../../src/machine/core/arena.rs)) for the frame's child-scope readers, which alloc into
the cart region and return the result up-stack — the shape the keystone's `for<'b>` `open` forbids by
construction. It is the transitional borrow-bounded accessor that lets a re-anchored reference ride up
the dispatcher call stack. Once the [scope-read migration](scope-reads-to-open.md) inverts those
readers, its only justification is gone, but `attach` and its externally-witnessed read path still
exist as a second access verb beside [`open`](../../src/witnessed.rs). (Its self-witnessed twin
`Sealed::read` is already gone — deleted by the shipped value-read migration; this item clears
`attach` to reach the single-access-verb end-state.)

**Acceptance criteria.**

- `Sealed` exposes a single access verb, `open` (plus its consuming externally-witnessed twin): `attach`
  and the externally-witnessed witness-borrow read path are deleted, the self-witnessed `read` already
  deleted by the shipped value-read migration, and no call site references either.
- Any reader that provably cannot nest under `open` is surfaced and documented as the lone exception,
  not silently retained; no speculative generic borrow-bounded `attach` is added — the destination is
  open-only, and a fallback is built only if a concrete site forces it.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Open-only is the destination — decided.* A single access verb is the substrate's target surface;
  this item is the cleanup that confirms no consumer still needs the transitional one.
- *No speculative generic `attach` — decided.* The shipped `attach` is scope-specialized; rather than
  generalizing it to a `Sealed<T>` verb on spec, every site prefers `open` + copy-out, and the survey
  for an un-nestable non-scope reference happens here. A generic borrow-bounded
  `attach<'w>(&'w self, &'w W) -> Live<'w, T>` is added *only* if such a site is found, surfaced with
  why it cannot fold — never as a default escape hatch.
- *Gated on a clean residue — decided.* If a consumption path proved un-invertible and still holds an
  `attach`, that is surfaced here rather than silently retaining the verb; the residue is closed before
  deletion, not worked around.

## Dependencies

**Requires:**

- [Invert the scope-handle reads onto `open`](scope-reads-to-open.md) — clears the scope-read escapes
  (and the `reattach_ref_with` wrapper) that are `attach`'s only remaining callers.

**Unblocks:** none.
