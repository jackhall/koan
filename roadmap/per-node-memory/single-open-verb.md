# One region handle, one access verb

Confine the build-at-a-brand leaf behind a branded region handle — so a bare `&KoanRegion` has no
`alloc` and "every object is witnessed" is compile-enforced — and collapse the access surface to
`open`, deleting the last retypes outside Witnessed/Sealed.

**Problem.** The witnessed memory model is compile-enforced for allocation: every `alloc_*` lives on
the branded [`RegionBrand`](../../src/machine/core/arena.rs) handle, a bare `&KoanRegion` exposes none
(a `compile_fail` doctest pins it, arena.rs:90), `alloc_resident` is the tight in-module leaf, and
`SealedExtern::attach` is deleted. One retype still survives outside Witnessed/Sealed:
[`recouple_scope`](../../src/machine/core/scope_ptr.rs) re-anchors a per-call child scope's lexical
parent / root at construction (scope.rs:287, 327–328; kfunction.rs:85), routing the free-`'b`
[`reattach_ref_with`](../../src/witnessed.rs). While that re-anchor sits outside the abstraction, "an
allocated object is always witnessed" stays an audited convention at the scope seam rather than a
closed type rule.

**Acceptance criteria.**

- Region allocation is reachable only through a branded region handle; a bare `&KoanRegion` exposes no
  `alloc_*` (a `compile_fail` doctest pins it). Structural allocations still yield co-located `&'a`
  residents — but through the handle, not a bare reference — so no bare `&KoanRegion` can mint an
  un-witnessed terminal, and "always witnessed" is compile-enforced.
- `Sealed` / `SealedExtern` expose a single access verb — `open` (plus its consuming
  externally-witnessed twin); `attach` and the `reattach_ref_with` witness-borrow read path are
  deleted.
- `recouple_scope` is removed (or routes the brand), so `reattach_ref_with` has no caller and is
  deleted — no retype outside Witnessed/Sealed remains.
- `try_reset_for_tail` keeps its three Miri tests.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Compile-enforce the memory model — decided.* The leaf moves onto a **frame-lifetime** branded
  handle (`RegionBrand<'a>`, minted at region-open and threaded to the allocation sites) so "an
  allocated object is always witnessed" is a type rule, not an audited convention — no bare
  `&KoanRegion` alloc to slip through. Frame-lifetime, *not* a per-alloc `for<'b>` brand: a structural
  resident (a binding entry, a `Module`'s child `&Scope`) must outlive any one brand window, so it
  needs a real `&'a` — which only a frame-lifetime handle hands back. The residents stay co-located
  `&'a` (borrow == content == the region's lifetime); nothing escapes its backing.
- *Open-only is the destination — decided.* A single access verb is the substrate's target surface;
  `attach` is already callerless, deleted here with the `reattach_ref_with` read path it routes.
- *The scope re-anchor folds into a construction door — decided.* `recouple_scope`'s sites split by
  what they re-anchor. The same-region children (`child_inheriting`, scope.rs:287) and the functor
  capture (`with_binder_and_functor`, kfunction.rs:85) only **lengthen a borrow** of an already-`'a`-
  content scope — the callers hold the resident `&'a Scope<'a>` (the builtin ctx, builtins.rs:79/102/116),
  so tightening the constructor signatures to `&'a Scope<'a>` makes the store a plain coercion, no
  retype. The one genuine retype is the per-call frame child (`child_for_frame`, scope.rs:327–328), which
  **content-shortens** a longer-lived lexical parent into the fresh region's `'a` under `Scope`'s
  invariance; it builds through an **externally-witnessed construction door** — the dual of `yoke` that
  brands the fresh region and re-anchors the foreign parent at one `for<'b>`, then seals the child
  witness-less as its `SealedExtern<ScopeRefFamily>`. Both halves share one enabler — threading the
  active scope **co-lifetimed** (`&'a Scope<'a>`), which it already is at its source (`current_scope`,
  `with_scope`) — and the door is the two-ref `with_branded_pair`, routing the substrate's existing
  `for<'b>` brand (the same contract `open` rests on), which typechecks against the real invariant
  `Scope<'b>` with no new unsafe primitive. So `recouple_scope` deletes and `reattach_ref_with` goes
  callerless.

## Dependencies

**Requires:** none — its construction-witnessing prerequisite has shipped.

**Unblocks:** none.
