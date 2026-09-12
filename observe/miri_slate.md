# Miri audit slate

<!-- slate-fingerprint

-->

The canonical list of tests Miri's tree-borrows mode signs off on for the
modules the rewrite keeps — `memory`, `parse`, `source` and `type_lattice`.
Each test is a minimal-shape driver of one region-substrate discipline; the
slate passes when Miri reports zero process-exit leaks and zero UB across the
whole list. It runs on the default build: `python3 tools/miri.py`.

`src/` carries no `unsafe` of its own. Every `unsafe` these tests reach lives in
`workgraph`'s witnessed substrate — the bump allocator, the branded re-anchor,
the region release — and is pinned library-side by
[workgraph/observe/miri_slate.md](../workgraph/observe/miri_slate.md). What this
slate pins is the *safe* koan code that drives that substrate: a bump-hosted
table, a `ManuallyDrop` buffer of bump bytes, a scratch region shared with the
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
  allocator from the caller. The backing `unsafe` is `BumpAllocator`'s in `witnessed.rs`.
- `src/memory/slots.rs` — the slot array's buffer is a `ManuallyDrop` `BumpVec` whose bytes the
  region releases whole, and its three-state cells are written in place through a live shared
  array. No `unsafe` of its own; the buffer's is `BumpVec`'s.
- `src/parse/ast/program.rs` — the program-region storage doors: plain bump allocations whose
  destination brand discharges residence at compile time, over `BumpAllocator`'s `unsafe` in
  `witnessed.rs`. No `unsafe` of its own.
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

**Slot array over bump memory** ([src/memory/slots.rs](../src/memory/slots.rs)) — a fixed run of
three-state cells in one bump allocation, released with the program region.

- `a_commit_retires_its_own_claim`
  claim then bind in place: the two writes a binder makes to one
  cell, read back through the array.
- `conflicts_name_what_stands`
  refused writes against standing claims and bindings, so a refusal
  is proven to change nothing in the bump-hosted cell.

## Recent full-slate run durations

Prepended by `python3 tools/miri.py --log` on a clean run, trimmed to five.

<!-- slate-durations:start -->
- 2026-09-11: 40s — 7 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
