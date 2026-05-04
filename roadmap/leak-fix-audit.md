# Open issues from the leak-fix audit

Most leak-fix follow-ups landed (see [design/memory-model.md](../design/memory-model.md)). One remains.
It is deferred until after per-type identity ships because that refactor will likely
reshape the memory model — signing off on the current model before then would mean
redoing the audit.

- **Miri hasn't run.** `CallArena::new`'s heap-pin + lifetime-erasure transmutes match the
  existing `RuntimeArena::alloc_*` pattern, but neither has been validated under Miri. The
  closure-escape paths in particular cross several lifetime-erased boundaries; Miri is the
  cheapest way to prove the unsafe blocks are settled.

## Dependencies

**Requires:**
- [Per-type identity for structs and methods](per-type-identity.md) — that refactor will
  likely reshape the memory model, so signing off on the current model first would mean
  redoing the audit.

**Unblocks:**
- [Static type checking and JIT compilation](static-typing-and-jit.md)
