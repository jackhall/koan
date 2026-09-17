# Miri audit slate — cellgraph

The canonical list of tests Miri's tree-borrows mode signs off on for the
`cellgraph` crate's memory safety — the cell slab, the per-cell bump regions,
and the witnessed carrier seam that re-anchors an erased value at a step's
lifetime. Each test is a minimal-shape mirror of an unsafe site in the crate;
the slate passes when Miri reports zero process-exit leaks and zero UB across
the whole list.

A property the compiler already guarantees is not on the slate: drop glue runs
by ownership (the crate suppresses it only behind `DropFree`'s `const` assert
and a `ManuallyDrop` whose target has the same glue), a pointer built from a
reference is aligned and non-null, and no `&mut` can reach a live region under
a step's writer, because the step holds the region table shared. Tests of those
shapes stay in the suite as ordinary tests.

Sibling to [koan's own slate](../../observe/miri_slate.md) and
[workgraph's](../../workgraph/observe/miri_slate.md) — split because these
tests live in the `cellgraph` crate's own lib test binary, a separate
`cargo test` target from either. Not wired into `tools/observe_tests.py`'s
automated drift check (that stays scoped to koan's own `src/`): this is plain
documentation, kept current by hand, for a manual run per
[.claude/skills/miri/SKILL.md](../../.claude/skills/miri/SKILL.md).

## The slate

36 tests, grouped by the unsafe site each pins down. Names below are the exact
test identifiers; pass them after `--` in the Miri command, or run the whole lib
binary:

```
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p cellgraph --lib
```

**`retype` primitive — `Erased<'graph, T>`** ([src/reattach.rs](../src/reattach.rs)) — the single audited
lifetime-retype, a `transmute_copy` behind a `ManuallyDrop` (the one site `transmute`'s
associated-type size proof can't cover). It is reached through the two doors on `Erased`, and every
call site shortens a stored form to a lifetime the referents outlive. The tests store a family value
in a cell's continuation slot and read through it a step later: `enter` re-anchors the slot's value
at the step's `'here` as it builds the context, and the continuation door hands that value over.
The borrowing family is the load-bearing one, since its erased form holds a real reference across
the store.

- `graph::tests::the_continuation_comes_back_re_anchored_at_the_step_brand`
- `graph::tests::a_step_stores_the_successor_the_next_step_receives`

**Region storage and the operand re-anchor** ([src/region.rs](../src/region.rs),
[src/carrier.rs](../src/carrier.rs), [src/graph.rs](../src/graph.rs)) — the same `retype` primitive,
reached at the two doors a value with reach uses. `Erased::erase` forgets a region borrow where
`Active::new` holds a built value and `Erased::reattach` hands it back at a shorter one, in
`Active::into_value` for a read, so a bumped value's borrow survives a round trip through a
slot free of every step brand; the placement test is the load-bearing one, since its operand view is a real
`&u32` into *another* cell's chunks that the built value keeps. The third covers the case where
nothing detaches at all: a reattached borrow names a cell that is still live, and that cell keeps
allocating under it, so what Miri checks is that the retag a region takes at every allocation
leaves an already-issued chunk borrow alone.

- `graph::tests::values::a_value_allocated_in_the_executing_cell_reaches_only_that_cell`
- `graph::tests::values::placing_a_value_into_another_cell_mints_that_cell_a_hold_on_its_reach`
- `graph::tests::values::a_reattached_borrow_survives_the_live_region_it_names_growing_under_it`

**The sealed tier's detached storage** ([src/sealed.rs](../src/sealed.rs),
[src/graph.rs](../src/graph.rs)) — the same `retype` primitive again, at the one door whose
referents may no longer live in the slab at all. A sealed region's chunks move out of their slot
into a sealed cell, and a continuation stored before the seal keeps borrows straight into them; the
accessor re-anchors that value at the next step's brand, so what Miri checks here is that a `Bump`
moving into a sealed cell moves no chunk byte and that the pre-seal borrows stay valid across the
move. The first test is the load-bearing one — it reads a `&u32` out of storage whose cell recycled
several steps earlier.

- `graph::tests::sealing::a_stored_mask_trades_the_sealed_slot_for_its_id`
- `graph::tests::sealing::a_reach_that_names_two_sealed_regions_merges_their_ids_in_order`

**Absorbed storage** ([src/region.rs](../src/region.rs), [src/graph.rs](../src/graph.rs)) — a region
is a bundle of bumps, and a locality merge moves a whole `Bump` into another region's bundle while
borrows minted before the merge stay live. The same pointer stability the seal transition relies on
has to hold across a move in the other direction, and in both roles: the first test moves the
*source* bump — a continuation reads a `&u32` out of storage whose cell was absorbed into the
reading cell's own region — and the second grows the *target*, splicing a bump into a sealed cell a
live continuation already borrows into.

- `graph::tests::absorption::a_uniquely_held_cell_is_absorbed_into_its_holder_instead_of_sealing`
- `graph::tests::absorption::a_cell_with_a_single_sealed_namer_seals_into_it`

**Region bookkeeping — `Kept`** ([src/region.rs](../src/region.rs)) — a sealed cell's frozen-closure
memo is a run of ids written into the sealed cell's own bump and held as a raw pointer, read back
only through the `&self` door on the region that wrote it. It stands on the same argument the seal
transition does — a `Bump` moves without moving a chunk byte, and a region's bump is reset only
once the region has been taken apart — but in a
shape nothing else covers: the pointer is stored *inside* the same struct as the bump it names, so
what Miri checks is that moving the region and absorbing another one into it leave the run readable,
and that the chunk carrying it goes at the region's drop rather than outliving it.

- `region::tests::a_memo_survives_its_region_moving_and_absorbing`

**Thin runs** ([src/region.rs](../src/region.rs)) — `Writer::thin_run` lays a length header and
its elements down in one raw allocation, and `ThinRun` reads both back through the one pointer it
keeps. What Miri checks is the arithmetic and the provenance: the element pointer is derived from the
allocation's own pointer, never from a reference to the header, so it may span the run; the offset
lands elements aligned whatever the payload's alignment, including at length zero and for zero-sized
payloads; a fill that writes into the same bump mid-run leaves the run intact; and a run stays
readable after the region claims a new chunk.

- `region::tests::a_thin_run_reads_back_what_the_fill_wrote`
- `region::tests::a_thin_run_of_length_zero_is_empty`
- `region::tests::a_thin_run_of_zero_sized_payloads_reads_every_unit`
- `region::tests::a_thin_run_of_over_aligned_payloads_lands_them_aligned`
- `region::tests::a_thin_run_whose_fill_writes_into_the_same_region`
- `region::tests::a_thin_run_survives_the_region_growing_under_it`

**The at-rest carrier and the crossing's two brands** ([src/dormant.rs](../src/dormant.rs),
[src/graph.rs](../src/graph.rs)) — the same `retype` primitive at the doors a value crosses steps
through. A value put to rest keeps its borrows while its home cell's storage moves under it — into a
holder's region bundle, out to a sealed cell, or both in turn — and the redeem door re-anchors it at
a later step's brand, with the reach it hands back derived from wherever that storage ended up. The
first three are the three exits a home takes, and the fourth chains two of them; what Miri checks
across all four is that a borrow minted before any number of merges still names live chunks after
them. The fifth is the load-bearing one for the park: a dormant carrier whose home has been
reclaimed, and one whose sealed cell has retired, are moved into the door and refused, which is only
a valid move because the value rests as bytes rather than as a reference. The sixth is the
crossing's severing: a copied view is re-anchored at a brand unrelated to the destination's region
and deep-copied through the writer, while a pinned one is embedded, so both re-anchors run in one
build. The seventh carries two lifetimes through a keep and a redeem across a home that sealed: the
value nests a borrow of heap storage outside the graph under a region borrow, and what Miri checks
is that the retype moves the region borrow and leaves the `'graph` one naming the same live bytes.

- `graph::tests::values::push_completes_a_value_built_into_the_consumer_is_read_in_its_own_step`
- `graph::tests::values::pull_completes_after_the_producer_seals`
- `graph::tests::values::pull_completes_after_the_producer_is_absorbed_into_the_consumer`
- `graph::tests::values::a_dormant_carrier_forwarded_through_two_merges_is_still_found`
- `graph::tests::values::redeem_refuses_once_the_storage_is_gone`
- `graph::tests::prices::a_copied_view_is_readable_and_a_pinned_one_embeddable`
- `graph::tests::values::a_graph_borrow_in_a_kept_value_redeems_after_its_home_seals`

**The own-region brand** ([src/region.rs](../src/region.rs), [src/graph.rs](../src/graph.rs)) —
the same `retype` primitive at the two places that re-anchor at `'here`, the shared borrow of the
region table a step holds for its whole length: `enter`, for the continuation, and the own-cell
crossing, for a pinned view. A capture at `'here` is read back a step later,
once with the cell's own bundle grown by an absorption under it, and once with the pinned home
sealed out of the slab entirely: what Miri checks is that the storage a `'here` reference names
stays where it was across the moves a region makes between two of the cell's steps.

- `graph::tests::values::a_cell_reference_captured_by_the_continuation_reads_after_the_region_absorbs`
- `graph::tests::values::a_pinned_view_at_the_cell_brand_survives_its_home_sealing`

**The tree habitat's splice, copy and tombstone** ([src/tree.rs](../src/tree.rs),
[src/graph.rs](../src/graph.rs)) — the same `retype` primitive where the storage under a borrow
moves by a **splice** rather than by a merge: a dying tree cell's whole bump is taken into an
ancestor's bundle, and a dormant carrier keyed to it resolves through a tombstone chain to wherever
those bytes ended up. The first is the load-bearing one for the pledge: a destination stores a
continuation over a value living in its child's bump, the child dies, and the next step reads
through the continuation — which is only sound because the pin pledged the child to splice here. The
second reads a dormant carrier back after its home spliced and left a tombstone. The third is the
forced copy's other half: a value crossing to a sibling is severed and deep-copied, so the old
cell's bump reclaims outright while the copy stays readable. The fourth carries spliced bumps out of
the slab entirely — the root seals, and a holder reads a tree-homed value out of the sealed cell.

- `graph::tests::tree::a_splice_keeps_a_borrow_the_destination_already_holds`
- `graph::tests::tree::a_dormant_carrier_whose_home_was_absorbed_redeems_from_the_destination`
- `graph::tests::tree::a_reinstall_inside_a_tree_copies_the_hop_and_reclaims_the_old_one`
- `graph::tests::tree::a_tree_root_that_seals_carries_its_spliced_bumps`

**The scratch habitat's re-anchor** ([src/graph.rs](../src/graph.rs),
[src/region.rs](../src/region.rs)) — the same `retype` primitive at the second re-anchor `enter`
makes: the scratch half of a continuation, handed back at a fresh `'scratch` each step over a
second bump that is reset whenever no scratch half is at rest. The family is invariant — a
`Cell<&'cell u32>` beside a spine of borrows — and the test writes through the re-anchored `Cell`
and reads the write back a step later, so what Miri checks is an interior write through a value
retyped at a new brand each wake, scratch bytes that embed borrows of the cell's own region, and —
because a reset rebuilds the bump under Miri — that nothing reads a scratch byte after the `enter`
that handed the bump back. The third holds a parent's scratch across a child's bump splicing into
the parent's region; the fourth shares one scratch bump between a host and two tenants, whose reset
waits on all of them.

- `graph::tests::habitat::a_slab_cells_scratch_survives_parks_and_resets_once_unnamed`
- `graph::tests::habitat::a_tree_cells_scratch_survives_parks_and_resets_once_unnamed`
- `graph::tests::habitat::an_absorb_leaves_the_absorbers_named_scratch_alone`
- `graph::tests::habitat::a_tenants_scratch_is_its_hosts_and_waits_on_every_tenant`

**Tenancy — a second writer under a parked borrow** ([src/tenant.rs](../src/tenant.rs),
[src/graph.rs](../src/graph.rs)) — no unsafe site of its own, and a second reading of the re-anchor in `enter`:
for a tenant step the `'here` a continuation is re-anchored at is its *host's*, so two cells park
borrows into one bump and each appends under the other's. The first test embeds a host-homed borrow
through the tenant's own writer and reads through it a step later. The other two park the host over
a run while a tenant appends enough to claim a new chunk, for a slab host and a tree host: what Miri
checks is that another writer's append is growth and not movement — the retag a bump takes at every
allocation leaves a chunk borrow issued to a different cell alone.

- `graph::tests::tenancy::a_tenant_embeds_a_host_homed_borrow_through_its_own_writer`
- `graph::tests::tenancy::a_slab_hosts_borrow_survives_a_tenant_appending_under_it`
- `graph::tests::tenancy::a_tree_hosts_borrow_survives_a_tenant_appending_under_it`

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
- 2026-09-17: 284.76s — 149 tests, 0 leaks, 0 UB
- 2026-09-14: 222.81s — 123 tests, 0 leaks, 0 UB
- 2026-09-13: 215.91s — 115 tests, 0 leaks, 0 UB
- 2026-09-12: 172.91s — 115 tests, 0 leaks, 0 UB
- 2026-09-12: 237.58s — 109 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
