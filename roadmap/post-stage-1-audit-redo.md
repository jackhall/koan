# Post-stage-1 Miri audit redo

**Problem.** The leak-fix audit signed off the current memory model under
`-Zmiri-tree-borrows`: the [`dispatch::runtime::arena`
slate](../src/dispatch/runtime/arena.rs) plus the closure-escape, KFuture-anchor,
and TCO-replace tests run with zero UB and zero process-exit leaks. Module system
stage 1 will reshape that model — the type-identity carrier, the per-scope
module registry, and whatever scope-aware type-resolution path stage 0 lands —
which means the parts of the model the slate exercises are about to move. The
sign-off doesn't carry across structural change: any new unsafe site, any new
shape of arena re-entry, any new lift path needs to face the same Miri evidence
the current set does.

**Impact.**

- *Memory-model sign-off survives stage 1.* The slate gets re-run against the
  reshaped runtime, so the closure-escape + per-call-arena story stays
  evidence-backed rather than carried on prior assertion.
- *New unsafe sites (if any) get the same treatment.* Stage 1 may introduce
  new transmute or raw-pointer sites around per-module type identity; each one
  picks up a targeted Miri test alongside the audit slate.
- *Static-typing-and-JIT has a stable target.* The checker's lifetime story
  and the JIT's codegen contract both want a memory model that's signed off
  against the post-stage-1 surface, not the pre-stage-1 one.

**Directions.** None decided.

- *Slate carry-forward.* Re-run the existing 16-test audit slate plus the
  `alloc_object_redirects_self_anchored_value_to_escape_arena` regression test
  added in the cycle-gate fix. Append new tests when stage 1 introduces new
  unsafe sites.
- *Trigger.* Pin to "stage 1 ships" rather than scheduling speculatively —
  the slate isn't useful until the new memory model surface exists to test.

## Dependencies

**Requires:**
- [Module system stage 1 — Module language](module-system-1-module-language.md) —
  the type-identity carrier and per-scope module registry land there, and that
  surgery reshapes the memory model the audit signs off on.

**Unblocks:**
- [Static type checking and JIT compilation](static-typing-and-jit.md) — both
  the checker's lifetime story and the JIT's codegen contract want a stable,
  signed-off memory model to target.
