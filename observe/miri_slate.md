# Miri audit slate

<!-- slate-fingerprint

-->

The canonical list of tests Miri's tree-borrows mode signs off on for the
modules the rewrite keeps ([TEST.md § The pending rewrite](../TEST.md#the-pending-rewrite)).
Each test is a minimal-shape driver of one region-substrate discipline; the
slate passes when Miri reports zero process-exit leaks and zero UB across the
whole list. It runs on the default build: `python3 tools/miri.py`.

`src/` carries no `unsafe` of its own. Every `unsafe` these tests reach lives in
`cellgraph` — `Writer::fill`, the reattach seam, the region release — pinned
library-side by [cellgraph/observe/miri_slate.md](../cellgraph/observe/miri_slate.md),
or in `bumpalo`'s allocator under the bump tier. `Writer::thin_run`, under
`memory`'s knot, is pinned library-side: the knot is safe indexing over the
`ThinRun` handle, with no layout or retype of its own. What this slate pins is
the *safe* koan code that drives them: a bump-hosted table, write-once slots laid
down in a cell's region and read through a covariant view, a scratch arena shared with the
registry it serves, and a knot re-tied at a crossing's destination. Each anchor file is therefore whitelisted below, and the
fingerprint block stays empty.

The old runtime's slate is frozen beside this one in
[miri_slate_pending_rewrite.md](miri_slate_pending_rewrite.md) and runs only
through `python3 tools/miri.py --pending-rewrite`. Nothing moves between the
two: a test named there goes with the runtime it audits.

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
- `src/type_lattice/registry.rs` — the registry's node table is built over its region's bump
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
  destination's `Writer`, nesting `fill` inside `fill` with `text` between and embedding program
  nodes at `'graph`. No `unsafe` of its own; the backing `unsafe` is `cellgraph`'s placement doors
  and reattach seam, whose pin and keep paths its own slate pins.
- `src/knot/copy.rs` — a knot member's copy re-ties its whole knot through the destination's
  `Writer`: `thin_run` fills the node run while each function's closure run, each data node's
  resident and cell runs, each module's member run, each barrier's resident and every held value's
  deep copy are written into the same region. No `unsafe` of its own; the backing `unsafe` is
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
<!-- slate-audit-whitelist:end -->

## The slate

Test names, grouped by the discipline each pins. `python3 tools/observe_tests.py
slate` reads every `` - `name` `` line below; a name must match exactly one test
in the lib binary.

**Region-hosted type registry** ([src/type_lattice/registry.rs](../src/type_lattice/registry.rs)) —
the table, every node and every slice live in the registry's region; a relation's transient
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
- `the_table_is_laid_once_and_never_grows`
  the verdict table laid in the registry's bump on its first record, and a hundred keys through
  one two-slot bucket, every slot written back in place.

**Program-region AST** ([src/parse/ast/program.rs](../src/parse/ast/program.rs)) — every name
and every parts run the parser produces is written into program storage through the doors here, and
the storage releases the whole tree. No `unsafe` of its own; the backing `unsafe` is `cellgraph`'s
`fill`, which every door lands on.

- `the_flip_reaches_quote_and_eval_bodies`
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
region it came from is released.

- `a_copied_list_outlives_its_home`
  a list of a string list, a string-keyed dict and a quote crosses under a copy verdict, is kept,
  its home released, and redeemed in the destination's next step: every byte reads back and the
  quote is the parsed node.

**Knots in a cell's region** ([src/knot/copy.rs](../src/knot/copy.rs)) — a knot
member copied across a crossing, a function or a data node, re-ties its whole knot through the
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
  crossing, each function member bringing its whole knot with it, and every captured byte reads
  back.
- `a_copied_barrier_outlives_its_home`
  an opaque view's barrier over a closure-holding function crosses under a copy verdict, is kept,
  its home released, and redeemed: the barrier beside the node and the whole knot behind it are
  written at the destination while the copy's node run is still being filled, and the captured
  bytes read back.

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
- `a_recursion_asking_two_children_per_level_peaks_at_its_depth`
  a deep stack of births and releases: five hundred and eleven tree cells born one path at a
  time under one root work, each delivering into its parked parent's scratch run before it is
  released.

**Self-contained substrate** ([src/program/substrate.rs](../src/program/substrate.rs)) — program
storage, the registry's bump and the interner in `self_cell`'s owner, and in its dependent at
`'graph` the graph, a borrow of the registry, and the builtin table and the program record laid
down in program storage.

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

## Recent full-slate run durations

Prepended by `python3 tools/miri.py --log` on a clean run, trimmed to five.

<!-- slate-durations:start -->
- 2026-09-24: 175s — 23 tests, 0 leaks, 0 UB
- 2026-09-22: 106s — 22 tests, 0 leaks, 0 UB
- 2026-09-22: 108s — 22 tests, 0 leaks, 0 UB
- 2026-09-22: 79s — 21 tests, 0 leaks, 0 UB
- 2026-09-21: 84s — 20 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
