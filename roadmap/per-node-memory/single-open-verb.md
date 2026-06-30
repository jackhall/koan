# One region handle, one access verb

Confine the build-at-a-brand leaf behind a branded region handle — so a bare `&KoanRegion` has no
`alloc` and "every object is witnessed" is compile-enforced — and collapse the access surface to
`open`, deleting the last retypes outside Witnessed/Sealed.

**Problem.** After [every construction is witnessed](witness-at-construction.md), the build leaf
`region.alloc_object(…) -> &'b` is still reachable on any `&KoanRegion` — the bare reference scopes
expose — even though its only legitimate callers are inside a `yoke` brand. Nothing compile-prevents a
fresh bare-`&'a` alloc from reopening the hole. Two read-side retypes also survive outside the
abstraction: the scope-pointer collapse left
[`SealedExtern::attach`](../../src/machine/core/scope_ptr.rs) — a borrow-bounded `&'w Scope<'b>`
re-anchor (routing [`reattach_ref_with`](../../src/witnessed.rs)) — callerless, and
[`recouple_scope`](../../src/machine/core/scope_ptr.rs) still re-couples a per-call child's lexical
parent / root through the same `reattach_ref_with`.

**Acceptance criteria.**

- Region allocation is reachable only through a branded region handle that `yoke` / `merge` /
  `transfer_into` hand their closure; a bare `&KoanRegion` exposes no `alloc_*`, so a value cannot be
  allocated outside the Witnessed/Sealed abstraction — the memory model is compile-enforced.
- `Sealed` / `SealedExtern` expose a single access verb — `open` (plus its consuming
  externally-witnessed twin); `attach` and the `reattach_ref_with` witness-borrow read path are
  deleted.
- `recouple_scope` is removed (or routes the brand), so `reattach_ref_with` has no caller and is
  deleted — no retype outside Witnessed/Sealed remains.
- `try_reset_for_tail` keeps its three Miri tests.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Compile-enforce the memory model — decided.* The leaf moves onto a branded region handle so "an
  allocated object is always witnessed" is a type rule, not an audited convention — no bare
  `&KoanRegion` alloc to slip through.
- *Open-only is the destination — decided.* A single access verb is the substrate's target surface;
  `attach` is already callerless, deleted here with the `reattach_ref_with` read path it routes.
- *`recouple_scope` folds in here — decided.* Deleting `reattach_ref_with` entirely requires its
  construction-time scope re-anchor to go too; the per-call child's parent / root couple onto the
  brand (or the whole scope is built witnessed) so no `reattach_ref_with` caller remains.

## Dependencies

**Requires:**

- [Witness value carriers at their construction site](witness-at-construction.md) — the bare `&'a`
  alloc callers must all be witnessed before the leaf can be confined behind the handle.

**Unblocks:** none.
