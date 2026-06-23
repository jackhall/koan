# Split Scope into region and outer lifetimes

**Problem.** [`Scope<'a>`](../../src/machine/core/scope.rs) is generic in a single lifetime `'a`
that does three jobs at once: the scope's own region (`region: &'a KoanRegion`), its binding-table
contents (`&'a KObject`), and its lexical parent (`outer: BoundedScopePtr<'a>`). For a per-call
child scope, its **own** region (the freshly-minted `FrameStorage.region`) and its **parent** (the
caller's lexical scope, living in an ancestor region) have genuinely different lifetimes, but the
single `'a` forces them to unify. [`CallFrame::new`](../../src/machine/core/arena.rs) and
`try_reset_for_tail` reconcile this by erasing **both** to `'static` through two hand-written
`unsafe` sites: the region `pin_deref` (recovering a `&'static KoanRegion` from the heap-pinned
storage) and the outer-link `reattach_ref::<ScopeFamily>` (erasing the parent scope's borrow to
`&'static`). The child is then built as `Scope<'static>` and stored through `ErasedScopePtr`. Those
two `unsafe` sites exist *only* because the child's single `'a` cannot be both its region's lifetime
and its parent's at once.

**Acceptance criteria.**

- `Scope` carries the region/content lifetime and the lexical-parent lifetime separately, so a
  child scope's own region and its parent need no common lifetime.
- The per-call child scope is constructed at real (non-`'static`) lifetimes and erased once through
  the safe `ErasedScopePtr::erase`; the construction-time region `pin_deref` and the outer-link
  `reattach_ref::<ScopeFamily>` in `CallFrame::new` / `try_reset_for_tail` no longer exist.
- The binding-table lifetime stays invariant (interior-mutable over region references), so the
  split does not reintroduce a variance soundness hole.
- `cargo test`, `cargo clippy --all-targets`, and the full Miri slate are green.

**Directions.**

- *Lifetime scheme — open.* Recommended: `Scope<'r, 'o>` — `'r` the region/content lifetime (own
  region, bindings) and `'o` the lexical-parent (`outer`) lifetime, with the `'r`/`'o` outlives
  relation the nesting requires. Alternatives: keep one lifetime but store `outer` fully erased (an
  `ErasedScopePtr`-style handle, witness-bounded on read) so only the parent edge is split off; or a
  GAT-based encoding. Spike the variance behaviour and the call-site blast radius first — `Scope<'a>`
  is pervasive.
- *Binding-table invariance — decided.* The binding-table lifetime must stay invariant; whatever
  scheme is chosen may not relax it to recover variance convenience.
- *Outer-link storage — open.* Whether `outer` becomes a second branded handle keyed on `'o`, or
  stays a `BoundedScopePtr` with the content lifetime threaded out of the struct's primary `'r`.

## Dependencies

Builds on the shipped `ErasedScopePtr` consolidation (the safe once-erased child-scope construction
this splits the lifetimes for).

**Requires:** none — its prerequisite consolidation has shipped.

**Unblocks:** none.
