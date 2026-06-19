# FrameStorage self-reference via ouroboros

Replace the hand-rolled region↔child-scope self-reference in `FrameStorage` with an
[`ouroboros`](https://crates.io/crates/ouroboros) `#[self_referencing]` struct, so the loop is
compiler-generated instead of audited `unsafe`.

**Problem.** [`FrameStorage`](../../src/machine/core/arena.rs) is a self-referential struct held
in `Rc<FrameStorage>`: the per-call `Scope` is allocated *into* `self.region` and its
`region: &'a KoanRegion` field points back at that same allocation. The borrow checker cannot
express a field borrowing from a sibling field, so the loop is closed by hand — `CallFrame::new`
and `try_reset_for_tail` take `&storage.region` as a raw pointer and `pin_deref` it to `&'static`,
the child scope is stored as a `ScopePtr<'static>` with its brand dropped, and the free content
lifetime is fabricated back through the **`unsafe`** [`ScopePtr::reattach_unbounded`](../../src/machine/core/scope_ptr.rs)
(reached by `CallFrame::scope` / `scope_for_bind`). Soundness rests on a prose invariant (the
`Rc` heap-pins the region; the brand bounds the pointer) rather than a type the compiler checks.

**Acceptance criteria.**

- `FrameStorage`'s region↔child-scope self-reference is an `ouroboros #[self_referencing]` struct;
  no `unsafe` is written to recover the child scope from the region.
- `ScopePtr` (the unbounded-capable branded pointer) and `ScopePtr::reattach_unbounded` no longer
  exist; `CallFrame` holds no `scope_ptr` field.
- The two `CallFrame` region `pin_deref` sites (`new`, `try_reset_for_tail`) are gone.
- Every read of the child scope routes a closure accessor (`with_scope(|s| …)`), not a returned
  `&Scope`: `scope`, `scope_for_bind`, `scope_bounded`, and the scheduler-side
  `reattach_node_scope` / `current_scope` read paths it feeds. `#[not_covariant]` (forced by
  `Scope<'a>`'s invariance) emits only `with_child`, so no child borrow can escape the closure —
  including the reads currently captured into `'step` continuations (`DepFinish`, `CatchFinish`;
  see `runtime.rs` / `exec.rs`), which must re-acquire the scope by re-entering `with_scope`
  against a held frame `Rc` rather than capturing the borrow.
- TCO reuse (`try_reset_for_tail`) still passes its three Miri slate tests: round-trip,
  refuses-when-aliased, allows-reset-under-escaped-storage.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` are clean.

**Directions.**

- *Crate: ouroboros — decided.* Chosen over `selfref` (built for mutable cyclic graphs, more
  ceremony than a build-once dependent needs) and `self_cell` (lighter but the foil, not the pick).
  Fits the one-owner / one-dependent / build-once shape ~1:1.
- *Closure accessor `with_scope(|s| R)` — decided by the type system.* `Scope<'a>` is invariant,
  forcing ouroboros `#[not_covariant]`, so there is no free-borrow (`borrow_child`) option; access
  is always through the generated `with_child` closure.
- *Serving the decide/continuation layer's escaping scope reads — open.* The child scope is read
  not only through the 6 `.scope()` sites but through `scope_bounded` → `reattach_node_scope`
  (`Yoked` arm) → `current_scope`, which fans out to ~32 decide-layer consumers — several of which
  capture the returned `&Scope<'step>` into a boxed continuation (`DepFinish` / `CatchFinish`) that
  runs on a *later* step (`runtime.rs`, `exec.rs`). `#[not_covariant]` forbids any such escaping
  borrow. The migration must therefore restructure those reads: a continuation captures the frame
  `Rc` (owned, escapes freely) and re-enters `with_scope` at run-time to re-acquire the scope, and
  the unified `current_scope() -> &Scope` read surface either inverts to a closure form or splits
  from the `YokedChild` path (which keeps a raw `BoundedScopePtr` and still returns a reference).
  This is the load-bearing design question; cost not yet established. *Recommended: frame-`Rc`-carrying
  continuations + a `with_current_scope(|s| …)` decide accessor; spike the continuation rework first.*
- *Full `ScopePtr` deletion (not just `CallFrame`'s use) — decided.* `ScopePtr` also backs `Module`
  / `Signature` (safe branded `reattach()`, free `&'a`) and `NodeScope::YokedChild`
  (`erase_static` + `reattach_bounded`). Meeting "`ScopePtr` no longer exists" folds all three onto
  `BoundedScopePtr` (add `BoundedScopePtr::erase_static`; `reattach()`/`reattach_bounded()` → `get()`),
  the move `KFunction` already made. ~40 `child_scope()` / `decl_scope()` consumer sites narrow from
  a free `&'a Scope<'a>` to a receiver-bounded `&'step Scope<'a>`.
- *Keep `CallFrame` as the public shell over `Rc<FrameStorage>` — decided.* The `Rc` stays for
  shared ownership (escapees clone it; the TCO `Rc::get_mut` uniqueness check); ouroboros supplies
  intra-struct address stability. Both are needed.
- *`try_reset_for_tail` rebuilds the struct rather than mutating the borrowed owner — decided.*
  Maps the current fresh-`FrameStorage`-then-swap-under-`Rc::get_mut` reset directly; ouroboros's
  generated `Drop` preserves the region-before-outer order and escapee-holds-old-storage semantics.
- *Outer-link erasure and the escape redirect stay manual — decided.* Both are cross-frame
  (reached via the `outer: Rc` field / a sibling region), not intra-struct, so ouroboros (which
  models only the region self-ref) does not subsume them; `BoundedScopePtr` and `ScopeFamily`
  remain for the outer link.
- *Test-side `.scope()` migration helper — open.* Whether the `arena.rs` `#[cfg(test)]` sites
  (some deliberately alias `frame.scope()` to exercise aliasing under Miri) warrant a test-only
  assert-closure helper, or port inline.

## Dependencies

Builds on shipped substrate — the region unsafe-collapse work on branch
`scheduler-owns-carrier-reattach` (the store-side `erase_to_static` and `with_frame_interior`
`pin_deref` collapses) — not on any open roadmap item. Working notes, blast-radius measurement,
and cold-start implementation notes live in `scratch/frame-storage-ouroboros.md`.

**Requires:** none — builds on shipped region unsafe-collapse substrate.

**Unblocks:** none.
