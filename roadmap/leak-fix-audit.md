# Open issues from the leak-fix audit

Most leak-fix follow-ups landed (see [design/memory-model.md](../design/memory-model.md)).
Miri under tree borrows still aborts on a use-after-free in `Scheduler::execute` before
reaching the `arena.rs` sites the original audit listed, so neither the scheduler finding
nor those sites are settled yet.

The follow-up is deferred until after module-system stage 1 ships because that
refactor will likely reshape the memory model — signing off on the current model
before then would mean redoing the audit.

- **`Scheduler::execute` Done-path UAF (Miri-caught).** The local `scope` at
  [scheduler.rs:140](../src/execute/scheduler.rs) is anchored to `prev_frame`. In the
  `NodeStep::Done` arms at [scheduler.rs:175-208](../src/execute/scheduler.rs) and
  [scheduler.rs:223-236](../src/execute/scheduler.rs), `frame` (or `_frame`) is moved
  into the arm and dropped at arm end. The loop tail at
  [scheduler.rs:278](../src/execute/scheduler.rs) (`scope.drain_pending()`) then reads
  through the dangling reference. Surfaced by the closure-escape Miri tests after the
  Replace-path fix landed. Note that for a finishing slot there is no "next node" that
  needs the drained writes — the per-call scope dies with the frame — so the drain in
  this path is both unnecessary and unsafe. Fix shape: skip `drain_pending` when the
  slot completed and the scope died (or move drain inside each Done arm before the
  frame-drop point, mirroring the Replace fix). Re-run the full Miri slate afterwards.
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
