# Miri audit slate

<!-- slate-fingerprint

-->

The canonical list of tests Miri's tree-borrows mode signs off on for the
modules the rewrite keeps — `memory`, `parse`, `source`, `type_lattice` and `values`.
Each test is a minimal-shape driver of one region-substrate discipline; the
slate passes when Miri reports zero process-exit leaks and zero UB across the
whole list. It runs on the default build: `python3 tools/miri.py`.

`src/` carries no `unsafe` of its own. Every `unsafe` these tests reach lives in
`cellgraph` — `Writer::fill`, the reattach seam, the region release — pinned
library-side by [cellgraph/observe/miri_slate.md](../cellgraph/observe/miri_slate.md),
or in `bumpalo`'s allocator under the bump tier. What this slate pins is the
*safe* koan code that drives them: a bump-hosted table, slots laid down in a
cell's region and written through `Cell`, a scratch arena shared with the
registry it serves. Each anchor file is therefore whitelisted below, and the
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
- `src/memory/slots.rs` — the slot array's cells and its claim counter are laid down by
  `Writer::fill` at `'cell` and written through `Cell` under the region's shared borrow, released
  with the cell. No `unsafe` of its own; the backing `unsafe` is `cellgraph`'s.
- `src/parse/ast/program.rs` — the program-region storage doors: plain bump allocations whose
  destination brand discharges residence at compile time, over `bumpalo`'s `unsafe`. No `unsafe`
  of its own.
- `src/values/crossing.rs` — the crossing verb's deep copy lays a value down through the
  destination's `Writer`, nesting `fill` inside `fill` with `text` between and embedding program
  nodes at `'graph`. No `unsafe` of its own; the backing `unsafe` is `cellgraph`'s placement doors
  and reattach seam, whose pin and keep paths its own slate pins.
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

**Program-region AST** ([src/parse/ast/program.rs](../src/parse/ast/program.rs)) — every name
and every parts run the parser produces is bumped into program storage through the doors here, and
the storage releases the whole tree.

- `the_flip_reaches_quote_and_eval_bodies`
  a quoted binder form with sigil-nested sub-expressions, parsed twice into the program region
  and compared shape for shape — nested runs, cached form entries and the quote wrapper all bumped.
- `a_binder_forms_type_slot_admits_the_bare_parenthesized_spelling`
  the same form unquoted, so the top-level statement peel and the type-slot flip run over
  region-hosted parts.

**Slot array in a cell's region** ([src/memory/slots.rs](../src/memory/slots.rs)) — a fixed run
of three-state `Cell` slots and a claim counter laid down by `Writer::fill` in a one-cell graph,
released with the cell.

- `a_commit_retires_its_own_claim`
  claim then bind in place: the two writes a binder makes to one
  cell, read back through the array.
- `conflicts_name_what_stands`
  refused writes against standing claims and bindings, so a refusal
  is proven to change nothing in the region-resident slot.

**Values in a cell's region** ([src/values/crossing.rs](../src/values/crossing.rs)) — a composite
value copied across a crossing is rebuilt through the destination's writer and read after the
region it came from is released.

- `a_copied_list_outlives_its_home`
  a list of a string list, a string-keyed dict and a quote crosses under a copy verdict, is kept,
  its home released, and redeemed in the destination's next step: every byte reads back and the
  quote is the parsed node.

## Recent full-slate run durations

Prepended by `python3 tools/miri.py --log` on a clean run, trimmed to five.

<!-- slate-durations:start -->
- 2026-09-12: 16s — 8 tests, 0 leaks, 0 UB
- 2026-09-12: 16s — 8 tests, 0 leaks, 0 UB
- 2026-09-11: 40s — 7 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
