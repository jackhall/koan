# Values on memory

The rewrite's foundation: Koan's runtime values, built over `src/memory`'s
regions, frames and brands and nothing else.

**Problem.** The old runtime's values live in
[machine/model/values](../../src/machine/model/values.rs): a universal
`KObject` enum, the `Carried` / `Held` value-channel cells with their carrier
states, the container substrates, the `Rehomed` door and the coercion walk.
They were built above the scheduler and the machine rather than above the
memory module, so a value's representation records the scheduler's delivery
envelope, the escape seam's copy-or-pin cost model, and the dispatch layer's
needs (`NamedPairs`, `coerce_function_cell`) alongside what the value *is*.
`src/memory` ([memory.rs](../../src/memory.rs)) is the module the rewrite keeps
— storage profile, allocation brands, the per-call frame shell, program
storage, `ScopeId` — and it imports nothing back, but nothing sits on it
yet: the frame shell is generic over a family the old runtime supplies. Its
own suite is gated behind `pending_rewrite` for the same reason
([TEST.md](../../TEST.md#the-pending-rewrite)).

**Acceptance criteria.**

- A `values` module depends on `memory`, `parse` and `type_lattice` and on no
  scheduler or scope type; `memory`'s frame shell is instantiated by it and
  `memory`'s suite runs in the default slate.
- Every composite value — list, dict, record, function, module, tagged value —
  is region-resident and `Drop`-free, born through a brand-confined construction
  door; there is no per-value reference count and no runtime residence audit.
- A value carries its type as a memoized `type_lattice` node, and a type check
  against a value reads that node rather than walking the value's contents.
- Moving a value across regions is one verb whose cost the module states, and
  the copy-versus-pin decision is local to that verb.
- The parts of the old value layer shaped by dispatch or delivery —
  `NamedPairs`, the function-cell coercion, the delivery envelope — do not
  exist in `values`; whatever survives of them lands in the layer that needs it.
- `values` has its own unit suite and Miri slate entries, and both are clean.
- `src/values/README.md` is the module's design doc, written fresh rather than
  migrated from `old_design/`, and the module's top-of-file comment links it
  ([the doc partition](../../.claude/skills/documentation/SKILL.md)).

**Directions.**

- *`memory` rides `cellgraph`'s region — decided.* `memory` instantiates
  `workgraph`'s witnessed module today; the scheduler that replaces `workgraph`
  is built after this item and `workgraph` is deleted with it, so `memory` is
  re-hosted on a `cellgraph` cell's region directly. One region type per run,
  and the `substrate` alias file was written to make the swap a local rewrite.
- *One universal enum or per-kind types — open.* `KObject` made every
  consumer match on every variant; per-kind types with a small tagged union at
  the boundary is the alternative. Recommended: per-kind types, with the enum
  confined to what a scope binding or a scheduler delivery must hold.
- *Container storage — decided.* The `ContainerSubstrate` shape — one `Copy`
  wrapper over cells, a bump-hosted index, a stored reach the doors derive —
  is the realized pattern
  ([value-substrates.md](../../old_design/value-substrates.md)) and carries over.

## Dependencies

**Requires:** none — the rewrite's foundation; `cellgraph`, `memory` and the type lattice are shipped.

**Unblocks:**

- [Scheduler on cellgraph](scheduler-on-cellgraph.md) — a value is what a cell delivers, so the delivery protocol is shaped against it.
- [Scope on values and types](scope-on-values-and-types.md) — a binding table holds values and their types.
