# Miri audit slate

<!-- slate-fingerprint
lattice/src/types/component.rs: 1
-->

The canonical list of tests Miri's tree-borrows mode signs off on for koan's own
modules.
Each test is a minimal-shape driver of one region-substrate discipline; the
slate passes when Miri reports zero process-exit leaks and zero UB across the
whole list. It runs on the default build: `python3 tools/miri.py`.

`src/` carries no `unsafe` of its own. `lattice` denies it outside its test build but for one
read, the lifetime-erased member table of a sealed component, whose group below pins it. Every
other `unsafe` these tests reach lives in
`cellgraph` — `Writer::fill`, the reattach seam, the region release — pinned
library-side by [cellgraph/observe/miri_slate.md](../cellgraph/observe/miri_slate.md),
or in `bumpalo`'s allocator under `lattice`'s bump tier. `Writer::thin_run`, under
`memory`'s knot, is pinned library-side: the knot is safe indexing over the
`ThinRun` handle, with no layout or retype of its own. What this slate pins is
the *safe* koan code that drives them: a bump-hosted table, write-once slots laid
down in a cell's region and read through a covariant view, a scratch arena shared with the
registry it serves, and a knot re-tied at a crossing's destination. Each such anchor file is therefore whitelisted below; the
fingerprint block records the lattice's one site.

Command of record and triage workflow live in
[.claude/skills/miri/SKILL.md](../.claude/skills/miri/SKILL.md).

## Stale-group whitelist

Slate groups whose anchor file carries no `unsafe` because the test pins a
safe-code discipline over the substrate. `slate-audit` skips the stale-group
check for these paths only; new-unsafe and fingerprint-drift checks still fire.

A slate test earns its place — and a whitelist entry — only if it can catch a
memory error *no other slate test catches*. Do not whitelist a group just to
silence the stale-anchor check; delete a redundant test instead.

<!-- slate-audit-whitelist:start -->
- `lattice/src/types/tiers.rs` — a tier's node table is built over its region's bump
  (`bump_table`) and every interned node's slices are bumped into the same region, so nothing the
  lattice owns carries drop glue and the region releases it whole; the relations take a scratch
  allocator from the caller. The backing `unsafe` is `bumpalo`'s.
- `src/memory/slots.rs` — the slot array is a safe façade over `cellgraph`'s `Writer::once_run`:
  each slot holds its value erased and is read through a covariant view at a shorter brand. No
  `unsafe` of its own; the backing `unsafe` is `cellgraph`'s `once_run` read, the reattach seam.
- `src/parse/ast/program.rs` — the program-storage doors: plain writes whose destination brand
  discharges residence at compile time. No `unsafe` of its own; the backing `unsafe` is
  `cellgraph`'s `fill`.
- `src/values/crossing.rs` — the crossing verb's deep copy lays a value down through the
  destination's `Writer`, each composite after its children, writing a dict's string keys inside
  its key run's `fill` and embedding program nodes at `'graph`. No `unsafe` of its own; the backing `unsafe` is `cellgraph`'s placement doors
  and reattach seam, whose pin and keep paths its own slate pins.
- `src/knot/copy.rs` — a knot member's copy re-ties its whole knot through the destination's
  `Writer`: `thin_run` fills the node run while each function's closure run, each data node's
  resident and cell runs, each module's member run and each barrier's resident are written into
  the same region, over the finished copies of the values the knot holds. No `unsafe` of its own; the backing `unsafe` is
  `cellgraph`'s `thin_run`, `fill` and reattach seam.
- `src/scheduler/drain.rs` — the drain performs every birth and every death: it creates a tail
  successor, wakes its state out of its predecessor, and only then releases that predecessor, whose
  region goes back to the pool for the hop after. No `unsafe` of its own; the backing `unsafe` is
  `cellgraph`'s creation and release doors, its delivery doors and its reattach seam.
- `src/program/substrate.rs` — the substrate owns program storage and the registry's bump beside
  the graph and the borrows of them, and moves, runs across separate calls and drops as one value. No `unsafe` of its
  own; the backing `unsafe` is `self_cell`'s joined allocation, which lends the dependent a
  borrow of the boxed owner.
- `src/program/body.rs` — the body runner parks holding an activation at `'here` and wakes it at
  the next step's, while evaluations read the same slots through a covariant view at their own
  shorter brands and a module body's activation and the enclosing place are written into the
  running region mid-body. No `unsafe` of its own; the backing `unsafe` is `cellgraph`'s
  once-written run read, its reattach seam and its keep and redeem doors.
- `src/dispatch/evaluate.rs` — the evaluator asks for a call's slots as tenants of its own cell,
  builds the argument record from what they kept, and — its callee returning `Str`, so placed
  `Shares` — hops to the callee's frame as a co-tenant of the frame it finishes, every hop a tenant
  of one host, where the body runner's tail test hops `Fresh` frames over `Fresh` children. No
  `unsafe` of its own; the backing `unsafe` is `cellgraph`'s tenant creation and release, its
  keep and redeem doors and its reattach seam.
- `src/scope/header.rs` — a habitat half's region header is laid once, through its header door
  or the door `values` holds projected to its branch slot, and holds `Cell`s written after it is
  laid: the branch, a build cell's specialization table, renewed in place, and a specialization's
  own handle. A specialization writes its parent's table through its lent region's writer, a frame
  reads its specialization's handle through its lent region, and the drain reads the table through
  `peek` between steps. No `unsafe` of its own; the backing
  `unsafe` is `cellgraph`'s header reattach, its projected door and `peek`.
<!-- slate-audit-whitelist:end -->

## The slate

Test names, grouped by the discipline each pins. `python3 tools/observe_tests.py
slate` reads every `` - `name` `` line below; a name must match exactly one test
in the lib binary.

**Region-hosted type tiers** ([lattice/src/types/tiers.rs](../lattice/src/types/tiers.rs)) —
the table, every node and every slice live in the tier's region; a relation's transient
buffers live in a scratch region the caller hands it.

- `interning_and_relations_touch_no_heap`
  the whole door battery over three fresh regions
  (registry, scratch, host), each grown first so that any allocation inside the bracket is the
  lattice's own. Under Miri every bump-hosted node, slice and table bucket is written, read back
  through the relations, and released with its region.
- `constants_match_freshly_interned_nodes`
  every node kind interned once into a fresh region and
  compared against its fixed digest, so every kind's slice shape is bumped and released.
- `a_record_is_order_blind_in_identity_and_ordered_in_presentation`
  a registry whose own region
  doubles as its scratch: the relation's buffer and the nodes it reads share one bump.
- `the_loads_verdict_table_keeps_every_verdict`
  the load's verdict table grown in the registry's bump past four thousand keys, laid into a
  frozen slot run at the freeze, and every key read back through a run chain.
- `an_agent_tier_hands_out_nodes_that_never_move`
  a node laid alone in the agent tier's bump, then a thousand more through the same chain, which
  grow the tier's index; the first node's reference still reads where it was laid.

**Tied component** ([lattice/src/types/component.rs](../lattice/src/types/component.rs)) — a
sealed component laid in the registry's region over its relative member table, repointed once at
the absolute table its seal rewrote, and read through a lifetime-erased atomic pointer beside a fixed length.

- `a_seal_keys_each_component_as_it_would_alone`
  two components sealed in one group, each re-sealed alone onto its existing nodes, and every
  member's absolute schema read back through its component's table, the cyclic edge included.
- `a_frozen_load_reads_alike_from_two_threads`
  a load holding a record, a union, a function type and a sealed two-member group, frozen, then
  read from two scoped threads at once: every member table read through its atomic pointer.

**Program-region AST** ([src/parse/ast/program.rs](../src/parse/ast/program.rs)) — every name
and every parts run the parser produces is written into program storage through the doors here, and
the storage releases the whole tree. No `unsafe` of its own; the backing `unsafe` is `cellgraph`'s
`fill`, which every door lands on.

- `the_flip_reaches_quote_bodies`
  a quoted binder form with sigil-nested sub-expressions, parsed twice into the program region
  and compared shape for shape — nested runs, cached form entries and the quote wrapper all written.
- `a_binder_forms_type_slot_admits_the_bare_parenthesized_spelling`
  the same form unquoted, so the top-level statement peel and the type-slot flip run over
  region-hosted parts.

**Slot array in a cell's region** ([src/memory/slots.rs](../src/memory/slots.rs)) — a run of
write-once slots laid down through `cellgraph`'s `Writer::once_run`, each holding its value erased,
and read through the covariant `SlotView` at a brand shorter than the one it was bound at: the read
is `cellgraph`'s reattach, reached from koan's safe façade.

- `a_tree_child_reads_a_slot_bound_in_its_root_through_a_view`
  a slot bound in a root's region, its view kept at rest, redeemed by a
  tree child, parked in its continuation and read a step later.

**Values in a cell's region** ([src/values/crossing.rs](../src/values/crossing.rs)) — a composite
value copied across a crossing is rebuilt through the destination's writer and read after the
region it came from is released, and a pinned one's header is written through its `Cell` and read
in a later step.

- `a_copied_list_outlives_its_home`
  a list of a string list and a string-keyed dict crosses under a copy verdict, is kept, its home
  released, and redeemed in the destination's next step: every byte reads back.
- `a_copied_type_value_names_a_type_its_destination_resolves`
  a type value whose record type its home's branch minted crosses under a copy verdict, its home
  is released, and the destination reads the record's node: the copy re-laid it in the
  destination's branch rather than entering the home's by reference.
- `a_settled_bit_written_through_a_pinned_header_reads_in_a_later_step`
  pins interior mutation through a shared borrow of a header laid in another cell's region: a
  record laid in a home cell's region is pinned into a holder and its home sealed; the holder's
  step marks it settled through its `Cell<Weight>`, and its next step reads the bit and every
  byte of the record before both cells are released.

**Knots in a cell's region** ([src/knot/copy.rs](../src/knot/copy.rs)) — a knot
member copied across a crossing, a function, a quote or a data node, re-ties its whole knot through the
destination's writer, and is read through its edges after the region it came from is released.

- `a_copied_knot_outlives_its_home`
  a two-node knot of mutually recursive functions, one of them quantified and the other born for a
  registration, whose closures capture a string and a string list, crosses under a copy verdict, is
  kept, its home released, and redeemed in the destination's next step: each edge names a node of
  the copy, each typing record — the quantifier map and the registered shape — is re-homed through
  the destination's writer, and every captured byte reads back.
- `a_copied_ring_outlives_its_home`
  a self-referencing tagged value whose record holds a string cell and an anonymous list node
  naming the ring crosses under a copy verdict, is kept, its home released, and redeemed: every
  edge names a node of the copy and the string reads back.
- `a_copied_module_outlives_its_home`
  a module holding a string list, a two-function knot and a newtype handle crosses under a copy
  verdict, is kept, its home released, and redeemed: every member is rebuilt through the one
  crossing, the two function members through one copy of their knot, and every captured byte
  reads back.
- `a_copied_barrier_outlives_its_home`
  an opaque view's barrier over a closure-holding function crosses under a copy verdict, is kept,
  its home released, and redeemed: the whole knot behind the barrier is copied first, the barrier
  beside the node is written at the destination while the copy's node run is still being filled,
  and the captured bytes read back.
- `a_copied_quote_outlives_its_home`
  a quote whose `$` names bind its own binder and a string crosses under a copy verdict, is kept,
  its home released, and redeemed: the code beside the node and its bound run are written at the
  destination while the copy's node run is still being filled, the self-reference names the copy,
  and the string reads back.

**Cells the drain creates and releases** ([src/scheduler/drain.rs](../src/scheduler/drain.rs)) — a
tail hand-off waking its state across a release, a state kept in one cell and woken in another,
and a producer's result filed into a consumer that outlives it. What the other groups pin is one
cell's region; what this pins is the ordering between two.

- `a_hand_off_redeems_out_of_the_region_the_drain_reclaims_next`
  three hops at each placement: the veneer wakes each successor's state out of a predecessor the
  drain releases once that first step returns, and at `Fresh` that region comes straight back out
  of the pool for the hop behind it, so every carried byte is written where somebody else's was.
- `a_spawned_child_wakes_holding_the_state_its_spawner_handed_over`
  the veneer's wake: a spawner's `'here` state is lifted and kept in its own region, and the child
  redeems it and crosses it to its own `'here` on its first entry, at each placement — the child
  reads the very bytes its spawner built.
- `a_shares_call_returns_its_result_through_the_callers_storage`
  the carrier door: `finish_fresh` under `Keeps` builds the result in the consumer's region, keeps
  it, files the dormant, and the producer dies — and the consumer redeems it out of a producer that
  is gone.
- `a_consumer_parked_on_three_producers_wakes_once_when_the_last_slot_fills`
  three producers filling one receipt run, so the run's slots and the consumer's scratch habitat
  are written by cells that are released before the consumer reads them back.
- `a_cell_gathers_here_values_across_two_parks_and_builds_from_them_in_storage`
  the scratch state over both brands: a cell parks twice, and the run it lays down in the habitat
  carries values homed in the executing cell at `'here` across the second park, so the last step
  builds its result out of them with no crossing at the wake — what Miri checks is storage read
  through a run that is retyped at a fresh pair of brands each wake and whose own bytes are handed
  back under the result built from it.
- `a_child_links_its_spawners_branch`
  a `Fresh` child's branch links its spawner's, reads a type through the link and caches the
  spawner's node in its own table by reference: a table in one region pointing into another,
  released child first.
- `a_hop_reaches_what_its_births_home_read_through_its_link`
  a tenant hops `Fresh` with a list homed at its host, so the successor's new branch links the
  host's, which links the spawner's: a table in a third region reading both regions' nodes
  through two links, read by the habitat check after the tenant is released.
- `a_recursion_asking_two_children_per_level_peaks_at_its_depth`
  a stack of births and releases: tree cells born one path at a time under one root work, each
  delivering into its parked parent's scratch run before it is released — two levels, seven cells,
  under Miri.

**Self-contained substrate** ([src/program/substrate.rs](../src/program/substrate.rs)) — program
storage, the types' load and agent-tier bumps and the interner in `self_cell`'s owner, and in its
dependent at `'graph` the graph, and the builtin table and the program record laid down in program
storage, the record borrowing the type tiers.

- `two_programs_run_and_are_read_back_in_a_separate_call`
  two substrates returned from a helper, moved through a `Vec` into a `Box`, each running its
  program in one call and read back in another through a root work resumed from the view the top
  level left at rest in the root, then dropped: the owner's box, the table and record in program
  storage, and the graph and its root beside them all released.

**The top level on the scheduler** ([src/program/body.rs](../src/program/body.rs)) — the body
runner performing a whole program under the drain: top-level bindings written in the root's region
by a tenant of it, each evaluation a tree child of the root or a tenant of its frame, a recursion
deeper than the slab cap on tree cells, module bodies laid down inline, and a component tied from
parts its evaluations built — each binding read later through a covariant view of an activation at a
shorter brand than the one it was bound at.

- `a_whole_program`
  every unit kind in one program, on a one-cell slab, read back after the drain.
- `an_eager_part_is_supplied_by_site_in_one_wake`
  a cyclic data member whose two eager parts are evaluated at one park and tied by site.
- `a_call_binds_each_type_parameter_to_its_solution`
  a quantified lambda called twice: each frame reads the callee's quantifier map out of the knot's
  region, solves the group against the argument's carried type, and lays the solution down as a type
  value in its own region — a read across the two regions at every call.
- `a_lambda_returned_from_a_frame_keeps_its_captures`
  two lambdas born as a frame's last statement in their calling evaluation's region, each capturing
  the list its frame was handed: one region holding only what its lambda reaches, spliced into the
  root; one also holding a list its lambda never reaches, whose lambda's knot is re-tied in the root
  with its capture deep-copied and the region reclaimed; each called after, reading its capture
  where it now lies.
- `a_self_call_in_tail_position_holds_its_cells_constant_however_deep`
  a frame tails its last statement into the evaluator with a view of its own activation: the
  successor is a tenant of the frame's caller, so the crossing pins the frame's region before the
  frame is released, and the evaluation reads through that view and tails into the next frame —
  three hops under Miri, against two.

**Dispatch** ([src/dispatch/evaluate.rs](../src/dispatch/evaluate.rs)) — the evaluator selecting a
registration's function under a frame's contract and hopping to its frame, its argument record built
in a tenant's storage and crossed into the callee's birth.

- `a_keyworded_self_call_in_tail_position_holds_its_cells_constant_however_deep`
  a keyworded call as a frame's last statement selects its own overload by the carried type of a
  link read through a newtype, and the evaluation owing the frame's contract tails into the next
  frame with the argument record it built — three hops under Miri, against two.
- `an_instance_made_in_a_frame_is_called_where_it_lands`
  a quantified member of a two-member knot whose closure holds an edge to its sibling and a
  captured string, instantiated at a frame's last statement: its knot re-tied through the frame's
  writer with the edge rehomed to the sibling's value, handed back across the frame's crossing, and
  called after, every captured byte read back.
- `a_closure_captures_a_type_only_where_a_call_in_it_contributes_it`
  a closure born as a frame's last statement holding a type capture in its layout past its
  builder's captures — the enclosing call's binding of a type name its body never writes, read
  from the frame's activation into the closure run, and composed where its contract names it —
  handed back across the frame's crossing and compared where it lands with one born under another
  binding.
- `a_call_by_name_through_a_barrier_rebuilds_its_value_both_ways`
  an opaque view's function member called by name through its barrier, twice nested: a list
  re-tagged inwards into the callee's frame, and a list built there from a captured string
  re-tagged outwards across the barrier frame's crossing, each read back where it lands.
- `a_using_block_calls_a_surfaced_member_through_its_barrier`
  a `USING … SCOPE` block run as a frame over a view's surfaced members: a keyworded member behind
  a barrier reached by its key, over a value member the block reads, its result crossing the
  block's frame and the barrier both, every captured byte read back.

**Build tree** ([src/scope/header.rs](../src/scope/header.rs),
[src/values/habitat.rs](../src/values/habitat.rs)) — a header's interior cells written after the
header is laid: through the half's own door, through a borrower's lent region, and read back
through `peek` between steps.

- `a_region_header_is_laid_on_the_first_intern_and_holds_the_table_and_the_home`
  a slab cell's header laid by its first intern through the projected door, its branch, table
  and home written through `Cell`s, and read back through the same header at the next step.
- `a_table_decides_hits_free_entries_and_its_least_recently_born_victim`
  a fixed table laid in a slab cell's region, entries filled, stamped and rekeyed in place through
  `Cell` writes.
- `two_frames_under_one_key_borrow_one_specialization_through_their_lend`
  a specialization's own step laying its header and table in its region, its handle read by two
  frames through their lent region and by none that borrows no region.
- `a_missed_specialization_holds_its_bindings_nodes_and_a_frame_reads_them_after_the_asker_is_gone`
  a new specialization's entry, opened beside its asker, laying the binding's nodes from the
  asker's region into the agent tier, read by a frame after the asker's region is gone.
- `a_parent_past_its_bound_releases_its_least_recently_born_child`
  the drain writing a parent's books through `peek` between steps — a hit's stamp, a new child's
  entry and home — under the root's table and a specialization's own, its victims released and an
  entry rebuilt in place, and the graph emptied.
- `a_miss_releases_its_victims_subtree_leaf_first`
  a specialization with a child of its own released by a miss, leaf first, a frame born cold under
  the dead parent, and a test's eviction releasing a subtree.
- `an_evicted_specialization_a_shares_frame_borrowed_waits_on_its_hosts_scratch`
  a tenant lent a specialization's region parks, the specialization is evicted under it, and the
  tenant wakes and reads what it laid in the agent tier; the region goes only once the host's
  scratch resets.
- `a_shares_loop_seeks_its_specialization_once_and_a_second_park_finds_it_warm`
  a keyed `Shares` body adopting its invariant sites into the agent tier over its host's scratch at
  two parks, the second reading the types and verdicts the first laid.
- `a_keyed_shares_call_runs_beside_a_host_scratch_laid_before_it`
  a host's scratch branch laid by an unkeyed tenant's caching chain, then read by a keyed tenant's
  caching chain over the host's region and the agent tier.

## Recent full-slate run durations

Prepended by `python3 tools/miri.py --log` on a clean run, trimmed to five.

<!-- slate-durations:start -->
- 2026-10-22: 1174s — 46 tests, 0 leaks, 0 UB
- 2026-10-22: 1176s — 46 tests, 0 leaks, 0 UB
- 2026-10-22: 1173s — 46 tests, 0 leaks, 0 UB
- 2026-10-22: 1173s — 46 tests, 0 leaks, 0 UB
- 2026-10-22: 1223s — 46 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
