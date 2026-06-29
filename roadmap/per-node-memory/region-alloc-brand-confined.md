# Confine `Region::alloc` to a brand

Make allocation produce an erased carrier inside a rank-2 region brand rather than hand back a
region-lifetime reference, so no public `alloc -> &'a` remains and the alloc retype is brand-confined.

**Problem.** [`Region::alloc`](../../src/witnessed/region.rs) hands back a bare-arena `&'a K::At<'a>`
through the loose [`reattach_ref_with`](../../src/witnessed.rs) wrapper, re-anchoring the `'static`
store to the `&'a self` region borrow. Because `'a` is the free region-borrow lifetime — not a
`for<'b>` brand — the reference can escape to wherever `&'a self` is borrowed, and soundness rests on
the caller holding a pin, the residue the rank-2 `open` closes everywhere else. The witness-less
frame-builder site ([`arena.rs`](../../src/machine/core/arena.rs)) allocates the per-call child scope
and immediately erases it to a `SealedExtern` carrier, so the `&'a` is a transient the wrapper exists
only to mint.

**Acceptance criteria.**

- Allocation produces an erased carrier, never a live region-lifetime reference: it happens only inside
  a rank-2 region brand — the `yoke` / `merge` / `transfer_into` region closure (already this form), or
  a witness-less `alloc(value, |live| erase)` closure for the frame builder's pre-witness child scope.
- No public `Region::alloc -> &'a` remains; the alloc retype is confined by the brand, sound by the
  `for<'b>` quantifier exactly as `open`'s is.
- An aggregate composes its element carriers via `merge` / `transfer_into`, not by wiring siblings from
  separate flat alloc calls.
- TCO frame reuse is unaffected — `try_reset_for_tail` keeps its three Miri tests.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Brand-confined closure, not a free `&'a` — decided.* The alloc'd reference is born at an
  un-nameable `for<'b>` brand and only an erased carrier escapes, so the retype is sound by the brand
  (like `open`) — neither a free `&'a self -> &'a` (which lets the reference escape) nor a relabelled
  wrapper.
- *`yoke` is already the witnessed form — decided.* Witnessed construction already allocs inside
  `yoke`'s `for<'b>` region closure; this item routes the leaf retype through that brand and adds the
  witness-less closure for the one site `yoke` cannot serve — the frame builder, whose witness (the
  frame) does not yet exist — then deletes the public `alloc -> &'a`.

## Dependencies

**Requires:** none — a substrate-local rework of the allocator surface.

**Unblocks:**

- [`Sealed`: a single access verb](single-open-verb.md) — reworking `Region::alloc` clears its use of
  `reattach_ref_with`.
