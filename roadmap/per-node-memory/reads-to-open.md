# Migrate the consumption reads onto `open`

Restructure every read that escapes a re-anchored reference up the dispatcher stack onto `open` +
copy-out / CPS, deleting the transitional self-witnessed `read` and the loose witness-borrow wrappers.

**Problem.** With the object channel witnessed and its reach carried (the value-embedding sites `merge`
their delivered carrier and binds fold it into the scope reach-set) and the type channel converting next
([`alloc_ktype`](alloc-ktype-witnessed.md)), the
values flowing through slots are `Sealed` carriers, but the consumption sites still hand a re-anchored
reference back up the call stack — the shape the rank-2 [`open`](../../src/witnessed.rs) forbids by
construction. Three readers do it: the result-slot value reads ([`read_result`](../../src/scheduler.rs)
/ `read_result_with_frame`, routed through the transitional self-witnessed
[`read`](../../src/witnessed.rs)); the scope-handle reads (`scope_bounded` / `current_scope` /
`reattach_node_scope`) that carry an `&Scope` through `run_dispatch` / `SchedulerView`; and the ~40
loose [`reattach_with`](../../src/witnessed.rs) (25) / [`reattach_ref_with`](../../src/witnessed.rs)
(15) wrappers in the dispatch decide and `scope_ptr` paths. Until each copies out or inverts CPS, the
transitional `read` and both wrappers stay alive as alternate spellings of the one primitive.

**Acceptance criteria.**

- Every result-slot value read and scope-handle read that currently rides a re-anchored reference
  up-stack either copies the needed data out of the `open` closure or is restructured CPS so the
  consumption nests inside it; no consumption borrow escapes its access window.
- The ~40 `reattach_with` / `reattach_ref_with` sites read through `open` (copy-out where the value does
  not escape) or [`attach`](single-open-verb.md) (only where a site proves it must ride up-stack); both
  wrapper functions are deleted.
- The transitional self-witnessed [`read`](../../src/witnessed.rs) is deleted, leaving the value path on
  `open` alone.
- TCO frame reuse is unaffected — `try_reset_for_tail` keeps its three Miri tests.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Per-site copy-out vs CPS — open.* Each consumer chooses copy-out (a cheap value) or a continuation
  rewrite (a borrow-heavy path); decided site-by-site during implementation.
- *This item owns the `read` deletion — decided.* The self-witnessed `read` is retired here, the dual
  of [single access verb](single-open-verb.md) retiring the externally-witnessed `attach`; the two land
  the single-access-verb end-state together.
- *One PR across the wrappers — decided.* The ~40 `reattach_*` sites are a uniform mechanical change, so
  the two wrappers retire together rather than as a separate near-identical item.
- *Prefer `open`, reach for `attach` — decided.* Each site favours `open` + copy-out to minimize the
  `attach` residue [single access verb](single-open-verb.md) must clear, reaching for `attach` only
  where a reference genuinely escapes the access.

## Dependencies

**Requires:**

- [`alloc_ktype` returns `Witnessed`](alloc-ktype-witnessed.md) — the type half; with both done no
  construction reads a value out bare, so `read` can retire.

**Unblocks:**

- [`Sealed`: a single access verb](single-open-verb.md) — clearing the value- and scope-read escapes is
  the migration that must land before `attach` can be deleted.
