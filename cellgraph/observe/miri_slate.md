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

12 tests, grouped by the unsafe site each pins down. Names below are the exact
test identifiers; pass them after `--` in the Miri command, or run the whole lib
binary:

```
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p cellgraph --lib
```

**`retype` primitive — `Erased<T>`** ([src/reattach.rs](../src/reattach.rs)) — the single audited
lifetime-retype, a `transmute_copy` behind a `ManuallyDrop` (the one site `transmute`'s
associated-type size proof can't cover). It is reached through the two doors on `Erased`, and every
call site shortens a stored form to a lifetime the referents outlive. The tests store a family value
in a cell's continuation slot, take it back out inside `enter`, and read through it; the borrowing
family is the load-bearing one, since its erased form holds a real reference across the store. The
third is a leak check on the erased slot's drop glue: a slot's reclamation must run the held
family's `Drop`, which Miri's process-exit leak detector is what verifies.

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
the chunks go with it rather than outliving the slab. The fourth covers the case where nothing
detaches at all: a reattached borrow names a cell that is still live, and that cell keeps
allocating through `&mut` under it, so what Miri checks is that the retag a region takes at every
allocation leaves an already-issued chunk borrow alone.

- `table::tests::values::a_value_allocated_in_the_executing_cell_reaches_only_that_cell`
- `table::tests::values::placing_a_value_into_another_cell_mints_that_cell_a_hold_on_its_reach`
- `table::tests::values::a_held_cell_leaves_the_slab_at_its_death_and_its_record_goes_with_its_holder`
- `table::tests::values::a_reattached_borrow_survives_the_live_region_it_names_growing_under_it`

**The sealed tier's detached storage** ([src/sealed.rs](../src/sealed.rs), [src/table.rs](../src/table.rs))
— the same `retype` primitive again, at the one door whose referents may no longer live in the slab
at all. A sealed region's chunks move out of their slot into a record, and a continuation stored
before the seal keeps borrows straight into them; the accessor re-anchors that value at the next
step's brand, so what Miri checks here is that a `Bump` moving into a record moves no chunk byte and
that the pre-seal borrows stay valid across the move. The first test is the load-bearing one — it
reads a `&u32` out of storage whose cell recycled several steps earlier. The third is the tier's
teardown: the cascade drops two records at once, and the process-exit leak detector confirms both
storages go with them.

- `table::tests::sealing::a_continuation_reads_back_with_reach_derived_through_the_sealed_tier`
- `table::tests::sealing::a_reach_that_names_two_sealed_regions_merges_their_ids_in_order`
- `table::tests::sealing::reclaiming_a_records_last_holder_cascades_through_its_aggregate`

**Absorbed storage** ([src/region.rs](../src/region.rs), [src/table.rs](../src/table.rs)) — a
region is a bundle of bumps, and a locality merge moves a whole `Bump` into another region's
bundle while borrows minted before the merge stay live. The same pointer stability the seal
transition relies on has to hold across a move in the other direction, and in both roles: the
first test moves the *source* bump — a continuation reads a `&u32` out of storage whose cell was
absorbed into the reading cell's own region — and the second grows the *target*, splicing a bump
into a sealed record a live continuation already borrows into.

- `table::tests::absorption::a_uniquely_held_cell_is_absorbed_into_its_holder_instead_of_sealing`
- `table::tests::absorption::a_cell_with_a_single_sealed_namer_seals_into_it`

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
- 2026-09-05: 106.98s — 39 tests, 0 leaks, 0 UB
- 2026-09-05: 99.61s — 39 tests, 0 leaks, 0 UB
- 2026-09-05: 49.13s — 25 tests, 0 leaks, 0 UB
- 2026-09-05: 70.27s — 24 tests, 0 leaks, 0 UB
- 2026-09-05: 92.83s — 24 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
