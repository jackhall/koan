# Open issues from the leak-fix audit

Most leak-fix follow-ups landed (see [design/memory-model.md](../design/memory-model.md)).
A first Miri pass under tree borrows ran the closure-escape, recursive-tagged-MATCH, and
unanchored-KFuture tests; it caught a use-after-free before clearing the `arena.rs` sites
the original audit listed, so neither the new finding nor those sites are settled yet.

The follow-up is deferred until after module-system stage 1 ships because that
refactor will likely reshape the memory model — signing off on the current model
before then would mean redoing the audit.

- **`Scheduler::execute` Replace path UAF (Miri-caught).** The local `scope` at
  [scheduler.rs:140](../src/execute/scheduler.rs) is captured from the previous frame.
  The `NodeStep::Replace` arm at
  [scheduler.rs:242-270](../src/execute/scheduler.rs) drops `prev_frame` and installs
  `next_scope` on `self.nodes[idx]` for the next iteration, but never updates the local
  `scope` binding. The loop tail at
  [scheduler.rs:274](../src/execute/scheduler.rs) (`scope.drain_pending()`) then reads
  through the dangling reference. Tree-borrows reports it as a
  use-after-free on the previous frame's `Scope`. Stable tests pass because the freed
  memory hasn't been reused yet — the bug is latent. Fix shape: refresh the local
  `scope` to `next_scope` on the Replace path (or move `drain_pending` ahead of the
  drop). Re-running the full Miri slate after the fix is part of the audit.
- **`arena.rs` unsafe sites not yet validated.** Miri aborted on the scheduler UAF
  before reaching the six `arena.rs` sites the original audit named
  (`RuntimeArena::alloc_object` / `_function` / `_scope`, the `*_singleton` helpers,
  `CallArena::new`, `CallArena::scope`). They aren't exonerated — they were just never
  reached. Re-run after the scheduler fix.

The Miri command of record:

```
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test --quiet -- \
    closure_escapes_outer_call_and_remains_invocable \
    escaped_closure_with_param_returns_body_value \
    list_of_closures_escapes_outer_call_with_rc_attached \
    recursive_tagged_match_no_uaf \
    unanchored_kfuture_no_arena_borrow_does_not_anchor \
    unanchored_kfuture_with_arena_borrow_does_anchor
```

## Dependencies

**Requires:**
- [Module system stage 1 — Module language](module-system-1-module-language.md) — the
  type-identity carrier and per-scope module registry land there, and that surgery
  will likely reshape the memory model the audit is signing off on.

**Unblocks:**
- [Static type checking and JIT compilation](static-typing-and-jit.md)
