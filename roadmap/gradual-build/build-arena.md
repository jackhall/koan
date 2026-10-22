# The build arena

The drain's books over build cells outside the cell graph, keyed on load
content, with the evaluator reading what the build decided.

**Problem.** The build tree is a cellgraph tree cell driven by two side paths
of the drain: the load door enters the build root through a raw `enter` with
no `Step` ([`load.rs`](../../src/scheduler/load.rs)), and a missed
specialization is entered off the ready stack, synchronously, through
`enter_beside`, ending `Quiesced` as no other cell may
([`drain.rs`](../../src/scheduler/drain.rs), `seek`). Its one step ends
`enlist()` ([`specialize.rs`](../../src/program/specialize.rs)): it
settles nothing, and the evaluator re-derives the build's typing per read —
`bound_path`, retype-and-judge, ranking at a use no key covers. A frame's
typing context rides cellgraph's lend of a writable half, fixed at the
frame's creation ([tree cells](../../cellgraph/src/tree/README.md#death)),
so a `Shares` frame's lend is counted on its host's scratch until that scratch
resets and a lend can close a wait cycle no creation refuses. The drain
mutates the region header between steps through `peek`, a door cellgraph
documents as read-only, and the header is one record of several
modules' concerns — branch, table and home. A specialization is keyed on content
the run builds as readily as on the load's, so a loop handing a keyed callee a
module built each hop births a cell per hop and grows the agent tier by each
binding ([build storage](../../design/build-storage.md#a-build-cell)); and
each parent bounds its own children, so the live set is the root bound times
the nested bound to the nesting depth.

**Acceptance criteria.**

- Build cells live in a **build arena** the agent owns, outside the cell
  graph: each is one frontier class under one binding, keyed on every name
  its seam binds, each name bound to content the frozen load names — a type
  whose in-root bit is set, a type value naming one, or a module whose content
  tree is closed. A cell holds its facts, residual and table; its types rest
  in the agent tier and outlive it. The arena has parent links, a borrower
  count that drops at the borrower's release, tenants included, and one
  budget over the whole arena, evicted least recently used, leaf first. The
  cell graph holds run cells alone, and the drain reads no cellgraph door
  between steps.
- The drain keeps the books: a frame's request carries its key, binding and an
  image of the binding's types laid at no cell's brand; the books answer hit,
  pending or miss; on a miss the drain runs
  [the load's fact function](../../src/build/README.md#the-one-walk) in the
  cell with the agent's store and births the frame at the cell's rest; a second request
  on the same key joins the pending one. Nothing homed in a run cell is
  referenced by a build cell.
- A child cell reads its parent's facts through the arena's parent link, and
  a fact is settled once per (frontier class, binding).
- A frame whose binding, at its entry or at a statement seam, names run
  content births no cell: it runs the fact function in its own activation
  over the carried types, into its own tiers, once per binding event, and a
  carried type equal to the last hop's is one decision. A loop whose
  statement binds load content holds O(1) build cells, and the agent tier
  grows by distinct load-content bindings alone.
- The evaluator re-derives no typing the build decided. At every site it
  reads the recorded decision — the candidate list in rank order with its
  elimination table, the solution points, closed contributions, the tail
  decision, the denoted type, the instance solution, a path resolved closed
  under the module's content, the route (hop, seek or cold), the target type
  of a retype — and `bound_path`, `denoted`, ranking over closed candidates,
  re-solving a selected candidate's group and the overlap check at an `EVAL`
  of a written quote are gone. What runs at run time is admission of a
  *maybe* candidate against carried types, an unsettled `satisfies`, a
  barrier's crossing, the view door's fit over a tree the build left open,
  the key's digest and the re-stamp itself.
- The frozen load holds the facts of every unit whose frontier is empty, each
  unit's binder split and each use's narrowed candidate list, so an
  activation evaluates only its dependent units.
- Every run cell born under a binding carries its cell's reference for its
  life, tenants included, and reads decisions through it; no cellgraph lend
  carries a typing context.
- A refusal a cell reaches faults the asker at the resumption point, worded
  as the load words it.
- Live build memory is bounded by the budget, and the live set is linear in
  the frame stack's distinct build cells, not in nesting depth.

**Directions.**

- *The budget's unit — open.* Bytes of the arena, known at a cell's rest
  since the arena is a bump per cell, or cells. Recommended: bytes.
- *The residue — open.* Construct a `Fresh` loop whose statement binds an
  action's result carrying a per-hop minted carrier, with a `MATCH` arm
  reading `it` into a keyworded use whose candidate depends on the carrier.
  Expected: one decision per hop in the activation, free from the second hop
  where the carrier's digest repeats.
- *`EVAL` shapes — open.* Composed code is run content, so its shape and
  facts live in the run's halves, never the agent tier, with segments
  numbered per shape and a callable born in it keying under its shape. Built
  once per code value in the value's home region at its birth, or at each
  `EVAL` in the `EVAL` frame's region; the first must say what a code value
  crossing cells carries.
- *How a binding crosses — decided.* As an image: the asker lays the binding's
  types into an owned bump the drain carries with the request, and the cell's
  first step relays them into the agent tier.
- *The layers — decided.* The load decides every unit with an empty frontier,
  records each unit's binder split and narrows each use's candidate list; all
  three are the fact function's frontier bookkeeping.
- *A module bound by its digest — open.* A frame's activation binds a
  module by its digest tree alone, so the fact function run there reads a
  member or a view over it as unknown (`Tree::Digest`), weaker than the
  structural tree the load computes for a build cell's binding. Accept the
  weaker answers in the activation, or carry the module's structural tree
  with the binding.
- *Layer coverage — open.* How many sites a realistic program decides at the
  load is to be measured on a constructed one.
- *The `src/build` partition — open.* By what it produces: shapes, and facts
  by kind — typing, selection, class, key — with the README the overview and
  index.
- *Elaboration evaluating deterministic code — deferred.* To
  [code names](../metaprogramming/code-names.md): a cell has no step to park,
  so the sub-run it would need is that item's.
- *Cross-agent seeding — deferred.* To [agents](../rewrite/agents.md).

## Dependencies

[Type tiers](../../design/type-tiers.md) and
[the scheduler](../../src/scheduler/README.md#specializations) describe the
mechanisms this replaces; [build storage](../../design/build-storage.md) pins
the end state. A cell and an activation run
[the load's fact function](../../src/build/README.md#the-one-walk) over
[its store](../../src/build/README.md#the-store).

**Requires:** none.

**Unblocks:**

- [Run cells alone](../../cellgraph/roadmap/run-cells-alone.md) — cellgraph's
  build-tree doors go once no build cell is a cell.
- [Agents](../rewrite/agents.md) — the per-agent build arena.
- [Setting the build budget](bound-settings.md) — the budget as a setting.
