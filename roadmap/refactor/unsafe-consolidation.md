# Consolidate unsafe sites and prune the Miri slate

Audit every `unsafe` in `src/`, funnel the scattered sites behind the trusted
allocator boundary where possible, and drop Miri slate tests that no longer guard
a distinct unsafe surface.

**Problem.** `src/` carries 33 `unsafe` blocks and one `unsafe impl` across seven
files. They concentrate in the per-call-arena allocator —
[`src/machine/core/arena.rs`](../../src/machine/core/arena.rs) (27) and
[`src/machine/model/values/module.rs`](../../src/machine/model/values/module.rs)
(6) — with seven more scattered across
[`builtins/try_with.rs`](../../src/builtins/try_with.rs),
[`builtins/match_case.rs`](../../src/builtins/match_case.rs),
[`execute/scheduler/node_store.rs`](../../src/machine/execute/scheduler/node_store.rs),
[`core/kfunction.rs`](../../src/machine/core/kfunction.rs), and
[`core/kfunction/invoke.rs`](../../src/machine/core/kfunction/invoke.rs). No pass
has audited whether each scattered site is irreducible or could route through the
arena's trusted boundary, nor whether each still carries a current `SAFETY`
invariant. The Miri leak/UB slate — the canonical list in
[`TEST.md`](../../TEST.md), with run durations in
[`observe/miri_slate.md`](../../observe/miri_slate.md) — is sized to cover this
whole spread; whether some slate tests now exercise the same unsafe surface, and
could be pruned, has not been checked.

**Impact.**

- *A minimized, documented unsafe surface.* Every surviving `unsafe` site is
  either irreducible allocator-core code or carries a current `SAFETY` invariant;
  scattered sites that can route through a safe arena API do.
- *One trusted boundary for memory safety.* Soundness reasoning concentrates on
  the arena core rather than a scatter of independently-argued sites.
- *A leaner Miri slate.* Slate tests that no longer guard a distinct unsafe
  surface are dropped, shrinking the audit's wall-clock (tracked in
  `observe/miri_slate.md`).

**Directions.**

- *Audit scope — decided.* Inventory all 34 sites; for each, record what it does
  and its `SAFETY` invariant, adding the comment where missing. The arena/module
  allocator core is expected to stay unsafe (irreducible); the seven scattered
  sites are the consolidation target.
- *Consolidation target — open.* Whether a scattered site routes through a widened
  safe arena API, gets rewritten in safe Rust, or stays with a documented
  invariant. Recommended: case-by-case — prefer a safe arena method where the
  pattern repeats across sites, keep-and-document where the reasoning is genuinely
  local.
- *Slate prune — open, contingent.* Pruning is downstream of consolidation: a
  slate test drops only once no remaining unsafe surface depends on it, and the
  `if possible` is real — if consolidation removes no distinct UB surface, the
  slate stays. Validate each consolidation under Miri's Tree Borrows and re-run the
  full slate before and after (via the `miri` skill) to confirm coverage holds,
  then update `TEST.md` and `observe/miri_slate.md`.

## Dependencies

**Requires:** none — an audit depends on nothing.

**Unblocks:** none tracked yet.

A sibling refactor-hygiene audit to the codebase-wide naming and responsibility
audit; the two are independent. The slate-prune half touches the testing docs
(`TEST.md`, `observe/miri_slate.md`), handled through the `miri` and documentation
skills rather than source-only work.
