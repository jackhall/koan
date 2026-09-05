# Miri audit slate — cellgraph

The canonical list of tests Miri's tree-borrows mode signs off on for the
`cellgraph` crate's memory safety — the cell slab, the per-cell bump regions,
and the witnessed carrier seam that re-anchors an erased value at a step's
lifetime. Each test is a minimal-shape mirror of an unsafe site in the crate;
the slate passes when Miri reports zero process-exit leaks and zero UB across
the whole list.

Sibling to [koan's own slate](../../observe/miri_slate.md) and
[workgraph's](../../workgraph/observe/miri_slate.md) — split because these
tests live in the `cellgraph` crate's own lib test binary, a separate
`cargo test` target from either. Not wired into `tools/observe_tests.py`'s
automated drift check (that stays scoped to koan's own `src/`): this is plain
documentation, kept current by hand, for a manual run per
[.claude/skills/miri/SKILL.md](../../.claude/skills/miri/SKILL.md).

## The slate

6 tests, grouped by the unsafe site each pins down. Names below are the exact
test identifiers; pass them after `--` in the Miri command, or run the whole lib
binary:

```
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p cellgraph --lib
```

**`retype` primitive — `Erased<T>`** ([src/reattach.rs](../src/reattach.rs)) — the single audited
lifetime-retype, a `transmute_copy` behind a `ManuallyDrop` (the one site `transmute`'s
associated-type size proof can't cover). It is reached only through `Erased::reattach`, and the only
caller of that is `StepContext::continuation`, which shortens the stored `'static` form to the step
brand. The tests store a family value in a cell's continuation slot, take it back out inside `enter`,
and read through it; the borrowing family is the load-bearing one, since its erased form holds a real
reference across the store. The third is a leak check on the erased slot's drop glue: a slot's
reclamation must run the held family's `Drop`, which Miri's process-exit leak detector is what
verifies.

- `table::tests::the_continuation_comes_back_re_anchored_at_the_step_brand`
- `table::tests::a_step_stores_the_successor_the_next_step_receives`
- `table::tests::reclaiming_a_slot_drops_the_continuation_it_held`

**Region storage and the operand re-anchor** ([src/region.rs](../src/region.rs),
[src/table.rs](../src/table.rs)) — the same `retype` primitive, reached at the two doors a value
with reach uses. `Erased::erase` forgets a region borrow at the alloc site and `Erased::reattach`
hands it back at a shorter one, so a bumped value's borrow survives a round trip through a
lifetime-free slot; the placement test is the load-bearing one, since its operand view is a real
`&u32` into *another* cell's chunks that the built value keeps. The third is the region's own
teardown: a reclaimed slot drops its `Bump`, and the process-exit leak detector is what confirms
the chunks go with it rather than outliving the slab.

- `table::tests::values::a_value_allocated_in_the_executing_cell_reaches_only_that_cell`
- `table::tests::values::placing_a_value_into_another_cell_mints_that_cell_a_hold_on_its_reach`
- `table::tests::values::a_held_cell_stays_resident_until_its_last_holder_reclaims`

## Adding tests to the slate

Add a test to the slate when a new unsafe site lands — a transmute,
raw-pointer round-trip, interior-mutation pattern under a live shared borrow,
or a cycle shape that storage-side reasoning can't rule out. Tests are
minimal-shape mirrors of the unsafe operation, not end-to-end feature tests;
they fail when Miri reports UB or a leak, not on values. Register the test here
**before** the slate run, never after it.

When you add or remove a slate test, update the list above and re-run the slate
to confirm the count matches.

## Recent full-slate run durations

The five most-recent full-slate runs, newest first. Append a new entry on every
full-slate run and trim to five so this list stays bounded. Use the most-recent
entry as the baseline expectation when scheduling a run.

<!-- slate-durations:start -->
- 2026-09-05: 65.19s — 17 tests, 0 leaks, 0 UB
- 2026-09-05: 1.22s — 9 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
