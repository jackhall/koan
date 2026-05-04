# Open issues from the leak-fix audit

Most leak-fix follow-ups landed (see [design/memory-model.md](../design/memory-model.md)). Three remain.
Two are deferred until after per-type identity ships because that refactor will likely
reshape the memory model — signing off on the current model before then would mean
redoing the audit. The third is a confirmed reproducer that needs investigation in its
own right.

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

- **Recursive user-fn taking a `Tagged` parameter UAFs during teardown.** A user-fn that
  takes a `b: Tagged` argument and recursively calls itself with another `Tagged` value
  (≥ 2 levels of recursion) runs to completion correctly — `PRINT` output is right, the
  return value lifts cleanly — but segfaults during the test rig's drop sequence. The
  binary doesn't hit it; reproducible only through `cargo test` (with any writer:
  `std::io::sink()`, `Box::new(std::io::stdout())`, or a `SharedBuf`). Not a stack
  overflow — `RUST_MIN_STACK=33554432` doesn't help. Found while writing tests for
  transient-node reclamation but predates that work (confirmed on master).

  Minimal reproducer (in `cargo test`, segfaults; in the binary, prints "done"):
  ```
  UNION Bit = (one: Null zero: Null)
  FN (HOP b: Tagged) -> Any = (MATCH (b) WITH (
      one -> (HOP (Bit (zero null)))
      zero -> (PRINT "done")
  ))
  HOP (Bit (one null))
  ```

  Triggers:
  - User-fn parameter typed `Tagged`.
  - Body tail-calls itself with another tagged value (constructed in the branch
    body or pre-bound via `LET`; both fail).
  - At least one recursion step (so the per-call frame for the outer call drops
    while the inner call is still in flight, or its lifted return walks back
    through it).

  Non-triggers (these all complete cleanly):
  - Bool-typed `MATCH` self-recursion.
  - Mutual recursion `A → B` without `MATCH`.
  - `Tagged` `MATCH` with no recursion.

  Suspected location: the `lift_kobject` / `needs_lift` Tagged arm in
  [`src/execute/lift.rs`](../src/execute/lift.rs#L86-L98) `Rc::clone`s the inner
  `Rc<KObject>` instead of recursing when `needs_lift` returns false. If the
  `Rc<KObject>` was constructed from a `value.deep_clone()` where `value` came
  from a per-call arena that's about to drop, the `Rc` would survive the arena
  drop but the underlying `KObject` would not — heap-allocated by `Rc::new`,
  but if any descendant references arena-allocated data (which `Tagged` itself
  shouldn't, but a future composite payload could), the lift would silently
  alias dangling memory. Worth instrumenting the construct/lift path with
  `RuntimeArena::alloc_count` deltas to confirm.

  Open questions: why test-only (suggests Drop-order interaction with `Box<dyn
  Write>` or `RuntimeArena`'s own drop)? Does running with a custom panic
  hook + AddressSanitizer surface the offending pointer? Does the bug exist
  before "user types and some refactors" (commit 4b0c078) or did Tagged
  introduce it?

## Dependencies

**Requires:**
- [Per-type identity for structs and methods](per-type-identity.md) — that refactor will
  likely reshape the memory model, so signing off on the current model first would mean
  redoing the audit.

**Unblocks:**
- [Static type checking and JIT compilation](static-typing-and-jit.md)
