# Fold the frame-side scope reads onto `open`

Read a frame's own child scope through a brand-confined `open` rather than the borrow-bounded
`attach`, so the last callers of `SealedExtern<ScopeRefFamily>::attach` are gone and the frame-scope
reads stop handing an `&Scope` up a `&mut self` path.

**Problem.** The decide channel reads the active scope from the run-loop step `open` — the scope's
carrier is zipped into the step brand, so the dispatch decide receives `&Scope<'b>` from it (shipped).
The **frame-side** scope reads do not: [`CallFrame::scope`](../../src/machine/core/arena.rs) /
`scope_for_bind` / `scope_bounded` re-anchor the frame's child scope through `reattach_scope` → the
borrow-bounded [`SealedExtern<ScopeRefFamily>::attach`](../../src/witnessed.rs), handing an `&Scope`
back to callers on `&mut self` paths — `exec.rs`'s call-scope reads, `submit.rs`'s cart-chain checks
and per-call binds, `runtime.rs`'s scope close, `branch_walk.rs`'s arm scope, and `nodes.rs`'s
body-chain assembly. The submit-path
[`reattach_node_scope`](../../src/machine/execute/dispatch/ctx.rs) and the literal-classify free
`current_scope` (the `&mut self` arms in
[`literal.rs`](../../src/machine/execute/dispatch/literal.rs)) route the same `attach` through that
helper's `Yoked` arm — its `YokedChild` arm's `ErasedScopePtr` is
[scope-pointer-collapse](scope-pointer-collapse.md)'s. Until each reads through `open`, the
borrow-bounded `attach` and the `reattach_ref_with` it routes keep their callers, so
[`single-open-verb`](single-open-verb.md) cannot delete them.

**Acceptance criteria.**

- A frame's child scope is read through a `for<'b>` brand or by copying out the data the caller needs:
  `CallFrame::scope` / `scope_for_bind` / `scope_bounded` and their `exec` / `submit` / `runtime` /
  `branch_walk` / `nodes` callers hand no `&Scope` up a `&mut self` path.
- The submit-path `reattach_node_scope` and the literal-classify free `current_scope` read the scope
  through `open` (or copy out), not the borrow-bounded `attach`.
- `SealedExtern<ScopeRefFamily>::attach` (routed by `CallFrame::reattach_scope`) has no caller left —
  its deletion is [`single-open-verb`](single-open-verb.md), which this unblocks.
- TCO frame reuse is unaffected — `try_reset_for_tail` keeps its three Miri tests.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Fold onto a brand, not a per-reader borrow — decided.* The frame-side reads follow the decide
  channel: read the child scope at a `for<'b>` brand (or copy out a `Copy` field) so the borrow cannot
  ride up a `&mut self` path, rather than retaining the borrow-bounded accessor.
- *Brand source per read site — open.* A read consuming its scope in place takes a rank-2 closure; a
  read needing only a scalar (an id, a region) copies it out, decided site-by-site during
  implementation. Recommended: copy-out for the `submit.rs` equality / region checks, closure for the
  binding plumbing.

## Dependencies

**Requires:** none — the decide-channel fold shipped (its `&Scope<'b>` is read from the step `open`).

**Unblocks:**

- [`Sealed`: a single access verb](single-open-verb.md) — folding the frame-side reads clears the
  borrow-bounded `attach`'s remaining callers.
