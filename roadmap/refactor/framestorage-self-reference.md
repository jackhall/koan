# FrameStorage self-reference removal

Remove the hand-rolled region↔child-scope self-reference in `FrameStorage`, preferring to drop the
back-pointer outright over encapsulating the loop behind a generated `unsafe`.

**Problem.** [`FrameStorage`](../../src/machine/core/arena.rs) is a self-referential struct held
in `Rc<FrameStorage>`: the per-call `Scope` is allocated *into* `self.region` and its
`region: &'a KoanRegion` field points back at that same allocation. The borrow checker cannot
express a field borrowing from a sibling field, so the loop is closed by hand — `CallFrame::new`
and `try_reset_for_tail` take `&storage.region` as a raw pointer and `pin_deref` it to `&'static`,
the child scope is stored as a `ScopePtr<'static>` with its brand dropped, and the free content
lifetime is fabricated back through the **`unsafe`** [`ScopePtr::reattach_unbounded`](../../src/machine/core/scope_ptr.rs)
(reached by `CallFrame::scope` / `scope_for_bind`). Soundness rests on a prose invariant (the
`Rc` heap-pins the region; the brand bounds the pointer) rather than a type the compiler checks.
`Scope<'a>` is invariant — its binding table ([`Bindings`](../../src/machine/core/bindings.rs)) is
interior-mutable over region-lifetime references — so no lifetime scheme makes the child scope
covariant, and an encapsulation crate (`ouroboros` / `self_cell`) is forced to closure-only access.

**Acceptance criteria.**

- The region↔child-scope self-reference no longer exists: either the `Scope.region` back-pointer is
  dropped (the region threaded explicitly, so region-owns-scope is a plain arena borrow) or the loop is
  encapsulated in a self-referencing struct. Either way, no `unsafe` is hand-written to recover the
  child scope from the region.
- `ScopePtr` (the unbounded-capable branded pointer) and `ScopePtr::reattach_unbounded` no longer
  exist; `CallFrame` holds no `scope_ptr` field.
- The two `CallFrame` region `pin_deref` sites (`new`, `try_reset_for_tail`) are gone.
- A child scope and the region its values allocate into are obtained together through one confined
  accessor, never assembled from independent handles — so a scope cannot be paired with a non-owning
  region by accident.
- A child-scope read that escapes into a later-running continuation (`DepFinish` / `CatchFinish`)
  carries the frame `Rc` and re-acquires the scope at run time, rather than capturing a `&Scope` borrow.
- `Module` / `ModuleSignature` / `NodeScope::YokedChild` re-anchor through `BoundedScopePtr`; the ~46
  `child_scope()` / `decl_scope()` consumer sites read a receiver-bounded `&'step Scope<'a>`.
- TCO reuse (`try_reset_for_tail`) still passes its three Miri slate tests: round-trip,
  refuses-when-aliased, allows-reset-under-escaped-storage.
- The Miri slate shrinks as the scope-erasure `unsafe` disappears: the `ScopePtr::reattach_unbounded`
  sites (`CallFrame` lifetime erasure) and the `NodeScope::YokedChild` fabrication / re-attach groups
  (`nodes.rs`, `dispatch/ctx.rs`, `runtime/submit.rs`) lose their backing `unsafe`, so their slate
  groups are pruned and `slate-audit` confirms no stale groups remain. (The value-carrier
  consolidation in [witnessed-carrier](witnessed-carrier.md) already retired the now-safe slot-read /
  `deps_at_step` / `apply_outcome` groups; this is the scope-path remainder.)
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` are clean.

**Directions.**

- *Removal strategy: drop the `Scope.region` back-pointer — open. Recommended: drop it.* Thread the
  region to its users (`alloc_ktype`, the seed binds at `scope.rs:443/492/526`) so region-owns-scope is
  a plain safe arena borrow with no dependency and the self-reference is *gone*, not hidden. Alternative:
  encapsulate the loop with `self_cell` (lighter) or `ouroboros` — both forced to `#[not_covariant]`
  closure access by `Scope`'s invariance. Spike the back-pointer-drop blast radius (the `self.region`
  users) before committing. Reopens the earlier ouroboros-is-decided choice.
- *Coherence guard for the dropped back-pointer — decided.* A confined `Active<'a> { region, scope }`
  newtype with a single `CallFrame::active()` constructor that pulls both from the same `FrameStorage`,
  so call sites receive the pair and cannot assemble a mismatched one. Compile-proof escalation to a
  `generativity` invariant brand is *deferred* unless confinement proves insufficient.
- *Continuation escaping-scope reads — decided.* The continuation captures the frame `Rc` (owned,
  escapes freely) and re-enters a scope accessor at run time; it never captures a `&Scope<'step>`
  borrow. Mechanism settled; the restructuring cost (the `current_scope` consumer fan-out) is not yet
  established.
- *Full `ScopePtr` deletion (not just `CallFrame`'s use) — decided.* Fold `Module` / `ModuleSignature`
  and `NodeScope::YokedChild` onto `BoundedScopePtr` (add `BoundedScopePtr::erase_static`; `reattach()` /
  `reattach_bounded()` → `get()`), the move `KFunction` already made.
- *Keep `CallFrame` as the public shell over `Rc<FrameStorage>` — decided.* The `Rc` stays for shared
  ownership (escapees clone it; the TCO `Rc::get_mut` uniqueness check).
- *`try_reset_for_tail` rebuilds the struct rather than mutating the borrowed owner — decided.* Maps the
  current fresh-storage-then-swap-under-`Rc::get_mut` reset directly; preserves region-before-outer drop
  order and escapee-holds-old-storage semantics.
- *Outer-link erasure and the escape redirect stay manual — decided.* Both are cross-frame (the
  `outer: Rc` field / a sibling region), not intra-struct, so neither the back-pointer drop nor an
  encapsulation subsumes them; `BoundedScopePtr` and `ScopeFamily` remain for the outer link.
- *Erased storage of the frame scope, if any, uses `Witnessed` — deferred.* If a path still needs the
  frame scope stored lifetime-erased, it routes the `Witnessed<ScopeFamily, Rc<CallFrame>>` carrier from
  the value-carrier consolidation rather than a bespoke pointer.
- *Test-side `.scope()` migration helper — open.* Whether the `arena.rs` `#[cfg(test)]` sites (some
  deliberately alias `frame.scope()` to exercise aliasing under Miri) warrant a test-only assert-closure
  helper, or port inline.

## Dependencies

Also builds on shipped substrate — the region unsafe-collapse work (the store-side `erase_to_static`
and `with_frame_interior` `pin_deref` collapses). Working notes, the back-pointer-drop blast-radius
measurement, the scope-mutation census, and the invariance / continuation spike findings live in
`scratch/framestorage-self-reference.md`.

**Requires:**
- [Witnessed carrier module for value lifetime-erasure](witnessed-carrier.md) — relocates the
  `Reattachable` / `ScopeFamily` substrate into the shared `witnessed` module and provides the
  `Witnessed<ScopeFamily, …>` frame-scope carrier.

**Unblocks:**
- [Split Scope into region and outer lifetimes](scope-region-outer-lifetimes.md) — the consolidated
  `ErasedScopePtr` is the substrate the once-erased child-scope construction builds on.
