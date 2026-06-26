# FrameStorage self-reference removal

Dissolve the region↔child-scope self-reference by making the per-call child scope an
externally-witnessed sealed carrier, deleting the three audited `unsafe` tokens that close
the loop by hand — without adding a self-referential-struct dependency.

**Problem.** [`FrameStorage`](../../src/machine/core/arena.rs) is a self-referential struct
held in `Rc<FrameStorage>`: the per-call `Scope` is allocated *into* `self.region` (a
`KoanRegion`) and the scope's `region: &'a KoanRegion` field points back at that same
allocation. The borrow checker cannot express a field borrowing from a sibling field, so both
directions of the loop are closed by hand, leaving three audited `unsafe` tokens whose
soundness rests on the prose invariant that the held `Rc` heap-pins the region:

- **Child-scope recovery** — the child `Scope` is stored as an `ErasedScopePtr` and recovered
  through [`ErasedScopePtr::reattach_witnessed`](../../src/machine/core/scope_ptr.rs); the frame
  `Rc` is passed as the witness, so the re-anchored *lifetime* is compiler-bounded and the
  residual `unsafe` is the `NonNull::as_ref` deref. This `as_ref` is a read-side deref of a scope
  that lives *in the arena* (the carrier holds a pointer, not the scope inline), and the same
  `reattach_witnessed` serves the `YokedChild` reader — so retiring it is not a property of the
  frame restructure alone (see Directions).
- **Region re-exposure** — [`CallFrame::with_frame_interior`](../../src/machine/core/arena.rs)
  re-exposes the same region at a free `'a` for the seed binds through `pin_deref(self.region())`.
- **The `pin_deref` primitive** — `reattach.rs`, the `&*ptr` home the region re-exposure routes;
  its sole caller is `with_frame_interior`.

The ~73 scope-handle reads (`scope_for_bind` / `scope_bounded` / `current_scope` /
`reattach_node_scope` / `reattach_witnessed`) and the ~65-site `ErasedScopePtr` /
`BoundedScopePtr` surface all route these tokens.

**Acceptance criteria.**

- The per-call child scope rides an externally-witnessed [`Sealed`](runloop-cps-open.md) carrier —
  erased into the frame's own region, re-anchored at access against the `CallFrame`'s held `Rc`
  through the keystone's `open` (or [`attach`](externally-witnessed-attach.md) if a read cannot nest)
  — and the frame's own `reattach_witnessed` call (`arena.rs`) is gone.
- The two `pin_deref` `unsafe` tokens — the `with_frame_interior` region re-exposure and the
  `pin_deref` primitive it solely feeds — are deleted; the seed binds reach the region through the
  witnessed carrier instead.
- The `reattach_witnessed` `as_ref` is deleted: `ErasedScopePtr` stores a `&'static Scope` (via
  `erase_to_static`) recovered through `reattach_ref_with`, retiring the token for the frame child
  scope and the `YokedChild` reader that shares the carrier — so all three audited tokens are gone.
- No `ouroboros` (or other self-referential-struct) dependency is added; the region↔child-scope loop
  dissolves into the substrate rather than being machine-generated.
- TCO frame reuse preserves the `Rc::get_mut` uniqueness check — the child scope's carrier bundles no
  `Rc` clone — and `try_reset_for_tail` still passes its three Miri tests (round-trip,
  refuses-when-aliased, allows-reset-under-escaped-storage).
- Every read of the child scope routes the `Sealed` accessor, not a returned `&Scope`:
  `scope_for_bind`, `scope_bounded`, `current_scope`, and the scheduler-side `reattach_node_scope`.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` are clean.

**Out of scope.** This change targets the `FrameStorage` region↔child-scope loop. It does **not**
touch the cross-frame `BoundedScopePtr::get` `as_ref` and its sole-caller `reattach_ref`, which
recover the outer/captured/root link a single struct's self-reference cannot subsume. The
`YokedChild` carrier stays an `ErasedScopePtr` (a cross-node erasure outside this struct), but the
storage switch (`NonNull` → `&'static Scope`, see Directions) retires the shared `reattach_witnessed`
`as_ref` for it too; migrating the `YokedChild` carrier itself to `Sealed` stays out of scope.

**Directions.**

- *Substrate, not ouroboros — decided.* An `ouroboros #[self_referencing]` struct and the
  externally-witnessed sealed carrier are two resolutions of the same handle; folding the
  scope-pointer machinery into the substrate dissolves the tokens without the new dependency, so the
  substrate approach is chosen.
- *Externally-witnessed, not bundled — decided.* The child scope's carrier supplies its witness at
  access from the `CallFrame`'s `Rc`; bundling a clone would peg `FrameStorage`'s refcount and
  defeat the TCO uniqueness check `try_reset_for_tail` depends on.
- *Retire the `reattach_witnessed` `as_ref` by storing `&'static Scope` — decided.* The `as_ref` is
  a read-side deref of an arena-resident scope (the carrier holds a `NonNull`, not the scope inline),
  shared with the `YokedChild` reader. No `&mut Scope` exists in the crate (mutation is interior
  `RefCell`), so `ErasedScopePtr` stores a `&'static Scope` via the safe `erase_to_static` and
  recovers it through `reattach_ref_with` — no `as_ref` — retiring the token for the frame child
  scope *and* `YokedChild` at once (shared carrier storage). A proxy Miri spike — a `&'static` shared
  ref stored into a `typed_arena`, the arena grown 512× and interior-mutated through the stored ref
  under tree borrows — is clean (0 UB, 0 leaks), confirming the stored reference survives the arena's
  later allocations; the implementation re-runs the real scope/lift slate to confirm in situ.
- *Continuation / seed-bind reads — decided.* A value crossing the step boundary lifts through the
  shipped [`yoke`](../../src/witnessed.rs) / `merge` bundle (or
  [`transfer_into`](transfer-into-lift.md)), not by widening the scope's lifetime.
- *Test-side `.scope()` migration helper — open.* Whether the `arena.rs` `#[cfg(test)]` sites (some
  deliberately alias `frame.scope()` to exercise aliasing under Miri) warrant a test-only
  assert-closure helper, or port inline.

## Dependencies

**Requires:**

- [Consuming externally-witnessed `open` and the run-loop step restructure](runloop-cps-open.md) —
  supplies the externally-witnessed sealed form and `open` the per-call child scope reads through.

**Unblocks:**

- [Production witness impls and the `alloc` witness plumbing](alloc-witness-plumbing.md) — the
  restructure gives the production bundle site a witness handle to the value's owning frame.
- [Migrate scope-handle reads to `open`](scope-reads-to-open.md) — the scope-read consolidation
  rides this restructure.
