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
— the slot array, program storage, `ScopeId` — and it imports nothing back,
but nothing sits on it yet.

**Acceptance criteria.**

- A `values` module depends on `memory`, `parse` and `type_lattice` and on no
  scheduler or scope type; `memory`'s shapes are instantiated by it.
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

- *`memory` rides `cellgraph`'s region — decided, and its own item.*
  [Memory on cellgraph](memory-on-cellgraph.md) narrows the module to shapes
  over the cell's own region; this item instantiates those shapes.
- *One universal enum or per-kind types — open.* `KObject` made every
  consumer match on every variant; per-kind types with a small tagged union at
  the boundary is the alternative. Recommended: per-kind types, with the enum
  confined to what a scope binding or a scheduler delivery must hold.
- *Container storage — decided.* The `ContainerSubstrate` shape — one `Copy`
  wrapper over cells, a bump-hosted index, a stored reach the doors derive —
  is the realized pattern
  ([value-substrates.md](../../old_design/value-substrates.md)) and carries over.

## Dependencies

**Requires:**

- [Memory on cellgraph](memory-on-cellgraph.md) — a value is born through the narrowed module's doors.

**Unblocks:**

- [Scheduler on cellgraph](scheduler-on-cellgraph.md) — a value is what a cell delivers, so the delivery protocol is shaped against it.
- [Scope on values and types](scope-on-values-and-types.md) — a binding table holds values and their types.
