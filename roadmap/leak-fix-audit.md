# Open issues from the leak-fix audit

Most leak-fix follow-ups landed (see [design/memory-model.md](../design/memory-model.md)).
The `Scheduler::execute` UAFs are fixed; the audit slate now runs to completion under Miri
tree borrows with no undefined behavior. Miri's leak detector reports 18 leaks at process
exit, so the memory model isn't signed off yet.

The follow-up is deferred until after module-system stage 1 ships because that
refactor will likely reshape the memory model — signing off on the current model
before then would mean redoing the audit.

- **18 process-exit leaks reported by Miri across the audit slate.** With the scheduler
  UAFs gone, Miri completes every test in the slate but reports leaks at exit. Sample
  allocations span `Rc::new` (alloc/rc.rs:426), `RawVec` growth, and the per-call arena
  storage. Plausibly: `Rc<CallArena>` cycles between a frame and a lifted `KFunction`
  that captures a scope inside that arena, or a frame-holding slot whose forward chain
  never resolves and so the slot's frame never drops. Needs a per-test triage pass — run
  one test at a time and read the allocation site stacks to attribute each leak.
- **`arena.rs` unsafe sites: no UB observed but not exhaustively exercised.** Miri now
  reaches the six sites the original audit named
  (`RuntimeArena::alloc_object` / `_function` / `_scope`, the `*_singleton` helpers,
  `CallArena::new`, `CallArena::scope`) without aborting. The audit slate doesn't drive
  every code path through them, so they're not exonerated — they're just no longer the
  leading cause of audit failure. Sign-off needs targeted tests for the singleton
  helpers and the `CallArena::scope` re-borrow shape.

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
