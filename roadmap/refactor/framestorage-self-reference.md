# FrameStorage self-reference removal

Replace the hand-rolled region↔child-scope self-reference in `FrameStorage` with an
[`ouroboros`](https://crates.io/crates/ouroboros) `#[self_referencing]` struct, so the loop is
compiler-generated and the three audited `unsafe` tokens that close it by hand are deleted.

**Problem.** [`FrameStorage`](../../src/machine/core/arena.rs) is a self-referential struct held in
`Rc<FrameStorage>`: the per-call `Scope` is allocated *into* `self.region` (a `KoanRegion`) and the
scope's `region: &'a KoanRegion` field points back at that same allocation. The borrow checker
cannot express a field borrowing from a sibling field, so both directions of the loop are closed by
hand, leaving three audited `unsafe` tokens whose soundness rests on the prose invariant that the
held `Rc` heap-pins the region rather than a type the compiler checks:

- **Child-scope recovery** — the child `Scope` is stored as an `ErasedScopePtr` (a
  `NonNull<Scope<'static>>` whose lifetime is forgotten through the safe `erase`) and recovered
  through [`ErasedScopePtr::reattach_witnessed`](../../src/machine/core/scope_ptr.rs) (scope_ptr.rs:167),
  reached via `CallFrame::reattach_scope` and its `scope` / `scope_for_bind` / `scope_bounded`
  wrappers. The frame `Rc` is passed as the witness, so the re-anchored *lifetime* is
  compiler-bounded; the residual `unsafe` is the `NonNull::as_ref` deref.
- **Region re-exposure** — [`CallFrame::with_frame_interior`](../../src/machine/core/arena.rs)
  (arena.rs:512) re-exposes the same region at a free `'a` for the seed binds (MATCH / TRY `it` and
  `KFunction::invoke` params) through `pin_deref(self.region())`.
- **The `pin_deref` primitive** — [`reattach.rs`](../../src/machine/core/reattach.rs) `:26`/`:28`,
  the `&*ptr` home the region re-exposure routes; its sole caller is `with_frame_interior`, so it
  goes dead once that site is converted.

**Acceptance criteria.**

- `FrameStorage`'s region↔child-scope self-reference is an `ouroboros #[self_referencing]` struct;
  no `unsafe` is written to recover the child scope from the region.
- `CallFrame` holds no `scope_ptr` field; the child scope comes from the generated `with_child`
  accessor, and the `ErasedScopePtr::reattach_witnessed` `NonNull::as_ref` deref is no longer
  reached for the per-call child. (`ErasedScopePtr` itself remains for `NodeScope::YokedChild`, a
  cross-node erasure outside this struct.)
- `CallFrame::with_frame_interior` exposes the region through the ouroboros owner accessor; its
  `pin_deref` call is gone and the seed binds re-anchor at the accessor-provided borrow.
- The `pin_deref` primitive in `reattach.rs` has no callers and is deleted.
- Every read of the child scope routes a closure accessor (`with_scope(|s| …)`), not a returned
  `&Scope`: `scope`, `scope_for_bind`, `scope_bounded`, and the scheduler-side
  `reattach_node_scope` / `current_scope` read paths they feed.
- TCO reuse (`try_reset_for_tail`) still passes its three Miri slate tests: round-trip,
  refuses-when-aliased, allows-reset-under-escaped-storage.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` are clean.

**Out of scope.** This change reaches exactly the three tokens above — the complete set tied to the
intra-`FrameStorage` region↔child-scope loop. It does **not** touch: the `BoundedScopePtr::get`
`as_ref` (scope_ptr.rs:115) and its sole-caller `unsafe reattach_ref` (witnessed.rs:409), which
recover the *cross-frame* outer/captured/root link a single struct's self-reference cannot subsume
(see the *Outer-link erasure stays manual* direction); the generic `witnessed.rs` erase/reattach
substrate; the `Witness` marker axiom (arena.rs:82); and `lift`'s value-relocation reattach
(lift.rs:48). `ErasedScopePtr::reattach_witnessed` survives for `NodeScope::YokedChild`; only its
`CallFrame` caller is removed.

**Directions.**

- *Crate: ouroboros — decided.* Chosen over `selfref` (built for mutable cyclic graphs, more
  ceremony than a build-once dependent needs) and `self_cell` (lighter, but the foil). Fits the
  one-owner / one-dependent / build-once shape ~1:1.
- *Closure accessor `with_scope(|s| R)` — decided by the type system.* `Scope<'a>` is invariant,
  forcing ouroboros `#[not_covariant]`, so there is no free-borrow (`borrow_child`) option;
  child-scope access is always through the generated `with_child` closure.
- *Reworking the decide/continuation layer's escaping scope reads — open.* The load-bearing
  question. The child scope is read not only through the `.scope()` sites but through
  `scope_bounded` → `reattach_node_scope` (`Yoked` arm) → `current_scope`, which fans out to ~34
  decide-layer consumers — several of which capture the returned `&Scope<'step>` into a boxed
  continuation (`DepFinish` / `CatchFinish`) that runs on a *later* step (`runtime.rs`,
  `outcome.rs`). `#[not_covariant]` forbids any escaping borrow, so a closure-confined `with_child`
  cannot serve a read whose result outlives the closure. The migration restructures those reads:
  the unified `current_scope() -> &Scope` surface inverts to a closure form
  (`with_current_scope(|s| …)`), and each scope-derived value that must escape into the step's
  `Outcome<'step>` is re-anchored `'b → 'step` by `witnessed::reattach_branded` (a zero-sized
  `PhantomData<&'step ()>` brand — see the dedicated direction below). The decide-path sites
  (`fn_value`, `single_poll`, `operator_chain`, `keyworded`) and the first continuation
  (`keyworded::finish`) are migrated this way; the remaining continuations
  (`field_list`, `runtime` `FinishCtx`, `exec` `BodyCtx`, `build_bare_outcomes`) follow the same shape.
- *Continuation re-anchor brand (`reattach_branded`) — decided, with a collapse obligation.* A parked
  continuation holds only a `for<'view>` view whose `'view: 'step` is not assumable, so it has no
  `'step`-lived borrow to hand the compile-enforced `reattach_with`. `reattach_branded` supplies the
  cart `'step` through a zero-sized `PhantomData` brand instead — zero runtime cost (a witness borrow
  is also never read), but *caller-asserted* (forgeable to `'static`) rather than borrow-bounded. It is
  the single audited brand for this, slate-tested under the `retype` group. The flip must re-evaluate
  it: collapse to a borrow-bounded `reattach_with` (or remove it entirely) if the `#[self_referencing]`
  storage can hand continuations a `'step`-lived scope directly, so the net effect on `unsafe` is
  negative rather than additive.
- *Region re-exposure in `with_frame_interior` — open.* Routing the region through the ouroboros
  owner accessor hands it at the accessor's borrow lifetime, not the free `'a` the seed binds use
  today (an `'a`-typed value deep-cloned into a younger per-call region). Removing the `pin_deref`
  therefore requires running the seed binds *inside* the region accessor closure and re-anchoring the
  caller-`'a`-typed bind values down to that borrow before `bind_value` — the same closure-inversion
  shape as the decide-layer rework. *Recommended: fold into the `with_*` accessor rework; re-anchor
  the seed binds through the witnessed reattach.*
- *Keep `CallFrame` as the public shell over `Rc<FrameStorage>` — decided.* The `Rc` stays for
  shared ownership (escapees clone it; the TCO `Rc::get_mut` uniqueness check); ouroboros supplies
  intra-struct address stability. Both are needed.
- *`try_reset_for_tail` rebuilds the struct rather than mutating the borrowed owner — decided.* Maps
  the current fresh-`FrameStorage`-then-swap-under-`Rc::get_mut` reset directly; ouroboros's
  generated `Drop` preserves the region-before-outer order and escapee-holds-old-storage semantics.
- *Outer-link erasure stays manual — decided.* The lexical `outer` link
  (`BoundedScopePtr`, scope_ptr.rs:115) is cross-frame — reached via the `outer: Rc` field / a
  sibling region — not intra-struct, so ouroboros (which models only the region self-ref) does not
  subsume it; `BoundedScopePtr` and `ScopeFamily` remain for it.
- *Test-side `.scope()` migration helper — open.* Whether the `arena.rs` `#[cfg(test)]` sites (some
  deliberately alias `frame.scope()` to exercise aliasing under Miri) warrant a test-only
  assert-closure helper, or port inline.

## Dependencies

Builds on the shipped arena unsafe-collapse substrate — the store-side `erase_to_static`, the
witness-bounded `ErasedScopePtr::reattach_witnessed`, and the real-lifetime per-call child
construction — all on `master`; not on any open roadmap item.

**Requires:** none — builds on shipped arena unsafe-collapse substrate.

**Unblocks:** none.
