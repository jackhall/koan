# Open issues from the leak-fix audit

Most leak-fix follow-ups landed (see [DECISIONS.md](../DECISIONS.md)). Two remain,
deferred until after per-type identity ships because that refactor will likely reshape
the memory model — signing off on the current model before then would mean redoing the
audit.

- **Miri hasn't run.** `CallArena::new`'s heap-pin + lifetime-erasure transmutes match the
  existing `RuntimeArena::alloc_*` pattern, but neither has been validated under Miri. The
  closure-escape paths in particular cross several lifetime-erased boundaries; Miri is the
  cheapest way to prove the unsafe blocks are settled.

- **KFuture conservative anchoring leaves room for tightening.**
  [`lift_kobject`](../src/execute/lift.rs)'s KFuture arm attaches the dying-frame Rc
  unconditionally because we don't track which arena each of `KFuture.bundle.args` and
  `KFuture.parsed.parts` came from. With per-descendant arena provenance this could
  become a `needs_lift`-style targeted attach. Non-issue today (KFutures don't escape as
  values), but worth revisiting alongside the async-features work that will make
  KFutures escape.

## Dependencies

**Requires:**
- [Per-type identity for structs and methods](per-type-identity.md) — that refactor will
  likely reshape the memory model, so signing off on the current model first would mean
  redoing the audit.

**Unblocks:**
- [Static type checking and JIT compilation](static-typing-and-jit.md)
