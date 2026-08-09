# Miri audit slate — workgraph

The canonical list of tests Miri's tree-borrows mode signs off on for the
`workgraph` crate's memory safety — the witnessed carrier substrate and the
generic region engine. Each test is a minimal-shape mirror of an unsafe site
in the crate; the slate passes when Miri reports zero process-exit leaks and
zero UB across the whole list.

Sibling to [koan's own slate](../../observe/miri_slate.md) — split because
these tests live in the `workgraph` crate's own lib test binary, a separate
`cargo test` target from koan's. Not wired into `tools/observe_tests.py`'s
automated drift check (that stays scoped to koan's own `src/`): this is plain
documentation, kept current by hand, for a manual run per
[.claude/skills/miri/SKILL.md](../../.claude/skills/miri/SKILL.md). Memory-model
invariants the slate verifies live in
[design/memory-model.md](../../design/memory-model.md).

## The slate

53 tests, grouped by the unsafe site (or the safe mint discipline routing it)
each pins down. Names below are the exact test identifiers; pass them after
`--` in the Miri command, or run the whole lib binary
(`MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p workgraph --lib`).

**`retype` primitive — `Erased<T>` / `Witnessed<T, W>`** ([src/witnessed.rs](../src/witnessed.rs))
— the single audited lifetime-retype every carrier family routes: `retype<A, B>` (a
`transmute_copy` behind a `ManuallyDrop`, the one site `transmute`'s GAT size-proof can't cover),
reached through `Erased<T>::erase` / `reattach`, the
consuming externally-witnessed `SealedExtern::open` (which reattaches a witness-less carrier — or a
`zip`-combined product / `seal_option` optional of carriers — at a generative `for<'b>` brand the
supplied witness pins), and through the `Witnessed` accessors: the rank-2 branded `with`
(borrow + read) and `map` (consume + transform), the borrow-bounded `read` that hands the carrier
*out* at the `&self` borrow — sound because its content lifetime is the borrow itself (not a free
`'b`), so the bundled `Witness` pins it for exactly that long — and the rank-2 branded composition
engine `merge_composed`, which re-anchors *two* carriers under one `'b`, runs a binding projection,
and re-seals under the composed witness (the descendant's ancestor-chain pin keeps both regions
live). The co-location-enforcing constructor `yoke` sources its carrier from the witness's region
through a `for<'b>` closure (no `unsafe` of its own — it routes the safe `erase`), so it is exercised
for the brand discipline, not a retype. The `unsafe impl Reattachable` families declare
layout-invariance and carry no runtime `unsafe` of their own — they are exercised through this
primitive using a generic `Rc<TestCart>` stand-in (this crate names no embedder family, so the
stand-in mirrors an embedder's own `Reattachable` families structurally: a covariant `&'r u32`, an
invariant `Cell<&'r u32>`, a `Box`-boxed non-`Copy` continuation, and the generic `And` product /
`OptionOf` optional families the `zip` / `seal_option` combinators seal). The tests erase a
borrow-carrying family to the `'static` store and
re-anchor it through every entry point — the witness-less helpers, the borrow-bounded `read` (read
after the original binding drops), and the `Witnessed` accessors that drop the *original* binding and
read back only through the bundled witness (the load-bearing case for the invariant `Cell<&'r u32>`
carrier) — plus `map`'s branded projection (binding a cart-coherent `&'b` value into the invariant
scope slot, the write `with` rejects). `yoke` sources a carrier from a stand-in cart's region, and the
composed merge binds an ancestor-cart ref into a descendant-cart scope at the shared brand — the
engine the envelope verbs run (Koan's `catch`/constructor sites route through them) — and re-seals
under the composed witness (read back after both call handles drop), plus a two-member-set check over
unrelated carts.
`SealedExtern::open` is exercised distinctly from the bundled `with` / `read`: a witness-less carrier
opened against a *separately-held* `Rc` witness (invariant `Cell<&'r u32>` read back after the
original drops), a **non-`Copy`** `Box<&'r u32>` consumed by the open (the boxed-continuation shape
`Copy`-bounded `Sealed::open` excludes), a **fat-pointer** `Box<dyn FnOnce>` continuation invoked
inside the brand (the retype over a two-word data + vtable pointer — the stored-continuation shape,
with tree borrows checking the capture read through the lifetime-fabricated box), and a
heterogeneous `zip` of a boxed carrier + a present `seal_option` optional + a reference opened
together at one brand (plus the `None`-optional arm). The escape-can't-compile guards are
`compile_fail` doctests on `with` / `map` / `yoke` / `SealedExtern::open`.

An embedder's realisation of the `unsafe trait` impls this primitive routes for — Koan's
`Witness` / `WitnessRegion` / `PinsRegion` for `FrameStorage`, backing the library's `RegionSet<F>`
(the unified region-owner witness `FrameSet` aliases, generic over the member trait in
`workgraph::witnessed::region_set`) — is covered cross-crate: its region-plus-`outer`-ancestry shape
is exactly what the `TestCart` stand-in mirrors, so `yoke_sources_carrier_from_witness_region` and
`compose_binds_ancestor_ref_into_descendant_scope` pin its yoke / merge / subsumption
(drop-an-ancestor-still-pinned-by-the-chain) UB shapes, and
`compose_keeps_unrelated_carts_as_a_two_member_set` the two-member-set case (a set witness always
represents the union — there is no failure verdict). Koan's `RegionSet::union` antichain logic
(union with `outer`-chain subsumption) is pinned separately by that embedder's own `frameset_*` /
`pins_region_walks_outer_chain` unit tests, which run under plain `cargo test` (no `unsafe` of their
own — the `unsafe` they exercise is this primitive).

- `erased_roundtrip`
- `read_borrow_bounded_witness_only`
- `branded_ref_reads_erased_store`
- `covariant_roundtrip_witness_only`
- `invariant_roundtrip_witness_only`
- `continuation_binds_cart_coherent_value_via_map`
- `invariant_same_brand_mutation`
- `yoke_sources_carrier_from_witness_region`
- `compose_binds_ancestor_ref_into_descendant_scope`
- `compose_keeps_unrelated_carts_as_a_two_member_set`
- `sealed_extern_open_externally_witnessed`
- `sealed_pinned_open_consumes_non_copy`
- `sealed_pinned_open_invokes_a_fat_pointer_continuation`
- `sealed_pinned_opens_beside_a_zipped_extern_operand`
- `seal_option_none_opens_to_none`

**Envelope mint — one pin bundle, home among its members** ([src/witnessed/delivered.rs](../src/witnessed/delivered.rs),
core in [src/witnessed/carrier.rs](../src/witnessed/carrier.rs)) — the reference-only `Carrier`
pins nothing; a value's liveness folds into a destination's minted set only at the
envelope-bearing verbs (`Delivered::transfer_into` / `open_adopted`, over `Carrier::compose_into` /
`ReachDescription::mint_resident`), out of the envelope's single owned `PinBundle` — its **home
region is an ordinary
member** of that bundle, so there is no separate residence channel and no residence mode. What a
relocation site chooses is the *source-pins claim*: the envelope's own pins (the product still
borrows into them) or the empty bundle (a true deep copy, whose producer must be free to die).
Seven tests pin the shape over a library-only profile (`RegionHost` frames, `u32` content), each
freeing every frame handle a regression would leave the value dangling into before the read: the
producer composing in from the envelope's pins and surviving the producer handle's drop; the
chained reach union surviving both content regions' drops (elements homed in the destination
itself, so the self rule strips the home member and the union alone carries the read — the strict
form, with no transitive pin to fall back on); the mixed-channel chain (one element
producer-hosted, one riding a reach set minted into a *reader* region foreign to the destination —
each fold must materialize the foreign host AND union the element reach, the multi-region aggregate
shape); the claimed-pins copy that still borrows; the
empty-claim **release** (the tail-turnover rule — a phantom member is the leak this gates, checked
by a `Weak` probe, and the exact regression an unconditional home-is-always-a-member fold would
produce); pass-through duplication (the reach set rides by reference, one `Rc` clone per
bundle member, no re-mint); and the single-seam re-stamp (`Delivered::restamp_in_place`, the
`finalize_terminal` `Disposition::Restamp` motion): the destination *is* the value's own home
region, so the description keeps home as an ordinary member while the self rule strips it from
every owned bundle — a kept self pin is a strong self-cycle the region never drops, the leak the
test's final `Weak` probe gates. The multi-handle mints here also pin `RegionHost::region`'s init-tag
re-derivation ([src/witnessed/host.rs](../src/witnessed/host.rs)): the minting call re-derives its
return through a plain `get`, so no caller ever holds the init frame's unique tag — which the next
foreign handle's interior arena write would disable. The only `unsafe` routed is the shared
`retype` (`alloc_resident`'s freeze-at-store and the branded re-anchors).

- `transfer_composes_the_source_home_from_its_pins`
- `transfer_unions_element_reach_across_folds`
- `transfer_chain_materializes_hosts_and_unions_reach_across_channels`
- `copied_transfer_pins_the_producer_when_the_product_still_borrows`
- `copied_transfer_releases_the_producer_when_nothing_borrows_it`
- `duplicate_shares_reach_and_clones_owned_pins`
- `restamp_in_place_keeps_home_in_the_description_but_pins_nothing_on_itself`

**The mint — the self rule / teardown** ([src/witnessed/reach.rs](../src/witnessed/reach.rs))
— the one cycle shape storage-side reasoning can't rule out: an owned bundle hosted in region A
holding `Rc<A>` would be a strong self-cycle A never drops. A mint splits description from ownership
on exactly this: the **description** keeps every composed member, A included, so membership stays
exact for a later lift; the **bundle** A retains — and the transit copy the threaded door hands on —
drops any member whose region is A's. The test mints a source that includes the destination's own
frame, checks the description names it while the bundle does not, and walks the teardown (A frees on
drop; the foreign member is released with the bundle, `Weak`-probed) — the Miri leak audit over this
test signs off the split-membership shape at the library layer. Embedder twin: koan's
`mint_teardown_releases_members` (over `FrameStorage` = `RegionHost`), whose refcount assertions
fail loud under plain `cargo test` and which stays off the Miri slate.

- `mint_keeps_home_in_the_description_but_not_the_bundle`

**A region's union bundle — one deduped antichain per region**
([src/witnessed/region.rs](../src/witnessed/region.rs), fold in
[src/witnessed/reach.rs](../src/witnessed/reach.rs)) — a value adopted copy-free into a region
(`Delivered::adopt_into`) leaves only a non-owning description behind, so the region itself owns the
pins: the mint's own retention folds into **one** `PinBundle` through `PinBundle::absorb`, dropped
whole at region death. The fold is where liveness can be dropped on the
floor, because `insert`'s subsumption deletes a member another member's owner chain already pins — a
wrong verdict frees a region the adopted value still borrows into (a UAF under tree borrows), a
missed dedup keeps an `Rc` nothing needs (a leak the exit detector catches). The test adopts the
same producer twice and then retains an ancestor of the already-retained member, asserts the
refcounts the antichain implies, and frees every producer handle before reading the adopted value
back under the region's bundle alone.

- `region_retention_folds_into_one_deduped_bundle`

**Region side tables — writes through `&self` under live readers**
([src/witnessed/region.rs](../src/witnessed/region.rs),
[src/witnessed/sectioned.rs](../src/witnessed/sectioned.rs)) — `Region::intern_reach_retained`
inserts into an `elsa::FrozenMap`, and `BumpAllocator::slice` bumps into a `bumpalo::Bump`, both
through a shared borrow while every reference a previous call handed out is still live. No `unsafe` of the crate's
own: the append-stable-address guarantees are the map's and the bump's.
But it is exactly the interior-mutation-under-a-live-shared-borrow shape tree borrows adjudicates,
and a violation is silent under the ordinary suite — a later insert that invalidated an earlier
entry's tag would poison every carrier referencing it. The tests interleave mints and reads over one
region, and the sectioned door does so at scale: one insert per cell, with each run's returned
reference held across every later cell's insert and read back after the build returns. The retention
fold rides the same `&self` — a miss borrows `retained_reach` mutably while those shared references
are live — and `hit_is_proof_the_region_already_pins` is the liveness half: it drops every handle on
a member the region pins only through an earlier mint's retention, so a hit that had skipped a fold
it still owed would be a use-after-free rather than a missing entry.

- `hit_returns_the_existing_entry`
- `empty_description_is_a_per_region_singleton`
- `hit_skips_the_retention_fold`
- `hit_is_proof_the_region_already_pins`
- `non_adjacent_runs_share_one_interned_description`
- `alternating_reach_degrades_to_length_one_runs`
- `equal_reach_inputs_cost_one_description_and_one_fold`
- `a_container_is_copy_and_drop_free`
- `project_bundles_a_cell_with_exactly_its_run_reach`

**`BumpAllocator` — a mutable collection over the region bump** ([src/witnessed/bump.rs](../src/witnessed/bump.rs))
— the crate's one `unsafe impl Allocator`. Its methods forward to `&Bump`'s own impl, so its safety
argument is the delegate's, but what a collection *does* through it is the crate's to answer for and
is a shape nothing above covers: the side-table group holds references handed out by an append-only
bump, while this one **reallocates**. `hashbrown` grows its buckets through `grow`, which bumpalo
satisfies **in place** when the old allocation is the newest one out of the chunk and by
allocate-copy-abandon when it is not — two different retaggings of a pointer the table keeps reading,
and a violation of either is silent under the ordinary suite because the abandoned bytes stay mapped
until region death. The test drives both paths: one table grown past several resizes, overwritten,
removed from and retained over, with a *second* table and bumped `&str`s interleaved so neither
table's bucket array is the newest allocation when it needs to grow, plus a vec grown and shrunk
over the same allocator.

It is a leak claim too, and the one a bump cannot fail loudly on — deallocation is a no-op here, so
a collection whose elements owned anything would read and write correctly and simply never free.
Embedders hold that line at their own declarations (koan's binding tables assert
`!needs_drop` on every key and value where each table is built); this test is the dynamic check that
the bump-side half — abandoned bucket arrays and vec buffers — is released with the chunks.

- `a_bump_backed_table_survives_growth_overwrite_and_removal`

**The three carrier states and the transform verbs between them**
([src/witnessed.rs](../src/witnessed.rs), [src/witnessed/delivered.rs](../src/witnessed/delivered.rs))
— one value family in three states (`Delivered` in transit, `Sealed` at rest, `Opened<'b>` in use)
connected by transform verbs, never by wrapping. The verbs move *ownership of pins* between
holders, so each is a place liveness can be dropped on the floor: `lift` upgrades a sealed
carrier's description `Weak → Rc` and unions home in (the test drops the description's hosting
arena and reads the value back — a missed upgrade is a dangling `Weak`, a UAF under tree borrows);
the adoption (`open_adopted`) mints into the destination, which retains the owned bundle there in
the same act (the test drops the producer and reads back under the destination's own pin alone); and
the round-trip test walks `Delivered → open_adopted → Opened → reseal → Sealed → open_at → Opened →
reseal → Sealed → lift → Delivered` with every intermediate handle dropped before the final read, so
only the chain of pins each verb hands the next keeps the value's region alive.

Three further shapes pin what the lift's `home` parameter and a region's union bundle are each
answerable for. The **degenerate reach** — a bump-hosted `Copy` pointee with an *empty* member set —
is the case where home is the whole of a value's liveness: nothing refcounts a `Drop`-free record
bumped into a region and reached through a reference-only carrier, so the lift's `Weak → Rc` upgrade
at the hosting region is all that stands between the envelope and a UAF once the declaring handle
goes (embedder twin: koan's declared operator-group registry entry, demoted to plain `cargo test`).
The **union chain** pins the other direction: a member resting in one region carries a reach naming a
foreign region, and the mint that froze it folded that region's owner into the member's region union
— so a second description, minted a region up, names the member's region *alone* and the foreign one
is reached transitively, through a union rather than a description (embedder twin: koan's stored
module value). And the **transitive root** is the lift's contract relaxation stated outright: `home`
must *cover* the description's hosting arena, not host it, so a lift whose home merely retains that
arena in its own union reads a hosted `&ReachDescription` two links away — every direct handle on
both the arena and what it names dropped before the lift runs (embedder twin: koan's `USING` window
overlay fold).

A further shape rides this group: an envelope dropped **by value** while its own bundle holds the
last `Rc` on the region its contents point into. The function-entry retag descends into the
by-value aggregate, so the in-call deallocation must not free memory carrying a protected tag —
which is what the dormant union slot supplies (retag does not descend into unions). Two tests drop
the envelope directly rather than splitting it with `into_parts`, and a third builds the shape
minimally.

- `lift_reowns_description_into_transit_bundle`
- `adopt_settles_resident_value_into_dest`
- `transform_verb_round_trip_preserves_liveness`
- `lift_of_a_bump_hosted_value_with_no_members_outlives_its_declaring_handle`
- `delivered_by_value_drop_frees_region_in_call`
- `a_regions_union_pins_what_its_own_members_reach`
- `lift_reads_a_description_hosted_under_a_transitive_root`

**The owned resting tier — `SealedPinned`** ([src/witnessed/dormant.rs](../src/witnessed/dormant.rs))
— a droppable family rests with its drop glue and its pins co-located, and the glue runs **before**
the pins (struct field order). A family whose destructor dereferences region memory is therefore
sound to drop unopened: the region is still pinned while its glue runs. The test drops the seal by
value as the last holder of its pointee's region, so it exercises the retag shape and the drop
order at once.

- `sealed_pinned_drop_runs_value_glue_before_pins`

**The scheduler's continuation slot — the owned tier in production shape**
([src/scheduler/nodes.rs](../src/scheduler/nodes.rs)) — a node's continuation rests as
`SealedPinned<W::Continuation, Rc<W::Frame>>`, sealed against the slot's anchor at the one install
door (`seal_work`, reached from `alloc_node` / `replace`) and opened once per step. The minimal
tier tests above pin the seal in isolation; these two drive it through the real `Scheduler`, where
the pin is an anchor `Rc` the scheduler *also* holds a second copy of on the dep row. A parked slot
torn down unopened must run its continuation's glue while the seal's own pin still holds the
region the continuation borrows into — `Scheduler`'s field order drops the dep row's anchor first,
so the seal's bundled pin is what remains. The round trip walks install → ready-queue pop →
`take_for_run` → `into_run_parts` → open → invoke, with every direct handle on the region dropped
before the open. The `unsafe` routed is the shared `retype` (through `SealedPinned::erase` / `open`)
with none of the scheduler's own.

- `parked_continuation_drops_under_its_own_pin`
- `parked_continuation_opens_and_runs_after_its_handles_drop`

**`StepContext::alloc_with` — finish-surface fold** ([src/witnessed/step_ctx.rs](../src/witnessed/step_ctx.rs))
— guarantee 5 made structural: every listed dep's envelope folds into the result's carrier by
construction, before the build closure can embed a dep view. The test's built value **is** a dep
view (a borrow into the producer's region, riding the result un-copied); the producer handle
drops, and the by-construction fold is the sole pin under the read. The behavioral twins
(`step_context_alloc_carrier_is_empty`,
`step_context_alloc_with_mints_dep_homes_and_preserves_dep_order` — membership and dep-order
assertions) run under plain `cargo test` and stay off the slate. Embedder twin: koan's
`record_retype_shares_substrate_across_producer_frame_free`, which drives the same combinator
(`alloc_carried_with`) over the `Record` substrate's shared borrow.

- `alloc_with_folds_dep_reach_before_result_read`

**`ReturnContract` re-attach — Done-boundary open** ([src/witnessed.rs](../src/witnessed.rs))
— an embedder's return-contract opens at its run-loop step brand alongside the continuation (a
`seal_option` optional operand of the step's `SealedExtern::open`), so it is live at the Done arm
with no reattach of its own; the `unsafe` lives in `SealedExtern::open` (`Erased::reattach`).
`erased_roundtrip` / `sealed_pinned_opens_beside_a_zipped_extern_operand` above pin it end-to-end
(Koan's `try_inside_tco_position_preserves_frame_chain`, in that embedder's own slate, exercises
the production shape). No separate minimal test here.

**`SealedExtern::open` — run-loop step-tail open** ([src/witnessed.rs](../src/witnessed.rs))
— the `unsafe { self.value.reattach() }` inside `SealedExtern::open` runs the transmute defined in the
`retype` group above with none of its own. An embedder's run-loop routes its step's continuation,
contract, and consumer-`dest` region together through this one call at a single generative `for<'b>`
brand its start cart pins. The `sealed_extern_*` / `sealed_pinned_*` minimal tests above pin it
directly; an embedder's
own scheduler-driving tests exercise it end-to-end. No separate minimal test here.

**The born doors — build-at-destination store, cross-region operand** ([src/witnessed/region.rs](../src/witnessed/region.rs))
— `RegionHandle::alloc_resident_born` and `alloc_resident_born_with` fuse construction and store at a
`for<'b>` brand, so the `unsafe` they route is the shared `retype` (through `erase_to_static` on the
store side and `SealedExtern::open` on the operand's re-anchor) with none of their own. The slate
family ([tests/born.rs](../src/witnessed/tests/born.rs)) is deliberately **invariant** in its region
lifetime — a node naming its own region, an optional parent in *another* region, and a
`Cell<Option<&'r str>>` — because a covariant stand-in would type-check under a weaker re-anchor and
prove nothing. The shapes: the stored value's region pointer is the destination's; an earlier store
reads back after 64 siblings append to the same typed cell (the arena's stable-address guarantee);
the returned reference accepts an interior-mutable write at the caller's own `'a` *after* the store;
a child born in one region embeds a parent resident in another under a pin held for the
destination's whole life; the same crossing store pinned by the **destination's own host** instead,
whose `outer` link to the parent is the whole of the operand's liveness once the caller's direct
handle on it drops (the frame chain as a pin — an embedder keeps only the innermost `Rc` and every
ancestor region rides the links; koan's `call_frame_chained_outer_frame_walkable` mirrors it over
`CallFrame` under plain `cargo test`); that child's region dies **first** with the pinned parent outliving it (the
production drop order, where a leak or a UAF is what a wrong pin duration looks like); a
three-region chain reads back through every hop; and a resident node erased to the witness-less
`SealedExtern` (the lifetime-free slot shape an embedder's scheduler stores) opens at a `for<'b>`
brand under the frame's own pin with the region growing through the born door *while the opened
reference is live* — one region under the re-anchored view and a sibling store at once. The
negative case is not a runtime rejection at
all — it is the `compile_fail` doctest on `alloc_resident_born`, where a value built over an ambient
region fails to coerce to `'b`.

- `the_born_door_stores_a_value_naming_its_own_region`
- `an_earlier_node_reads_back_after_its_siblings_are_stored`
- `the_returned_node_accepts_a_write_at_the_callers_lifetime`
- `the_born_with_door_embeds_a_parent_from_another_region`
- `the_born_with_door_accepts_the_childs_own_host_as_the_pin`
- `a_child_region_dies_before_the_parent_it_borrows`
- `a_chain_of_regions_reads_back_through_every_hop`
- `an_erased_node_opens_and_survives_a_sibling_store_inside_the_open`

**Doctest fixture markers** ([src/witnessed/doctest_fixture.rs](../src/witnessed/doctest_fixture.rs))
— the `unsafe impl Reattachable` for `RefFamily` / `InvFamily` and `unsafe impl Witness` /
`WitnessRegion` / `RegionOwner` / `PinsRegion` for `Cart` back the six `compile_fail` soundness
guards and their compiling twins (`cargo test --doc`), so a signature change to those traits has one
shared fixture to update instead of five pasted copies. Each impl is a marker with no runtime
`unsafe` operation of its own, asserting the identical `&'r u32` / `Cell<&'r u32>` layout-invariance
and owned-`Vec` fixed-address pin shapes the `retype` group's separate `TestCart` stand-in (in
`witnessed/tests.rs`, excluded from the audit as test scaffolding) already Miri-verifies. Doctests run
under `cargo test --doc`, not Miri, so there is no separate slate test here — the shape is pinned by
the `retype` group above.

## Adding tests to the slate

Add a test to the slate when a new unsafe site lands — a transmute, raw-pointer
round-trip, interior-mutation pattern under a live shared borrow, or a cycle
shape that storage-side reasoning can't rule out. Tests are minimal-shape
mirrors of the unsafe operation, not end-to-end feature tests; they fail when
Miri reports UB or a leak, not on values.

When you add or remove a slate test, update the list above and re-run the
slate to confirm the line count matches.
