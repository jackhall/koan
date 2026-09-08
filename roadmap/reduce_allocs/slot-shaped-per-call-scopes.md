# Slot-shaped per-call scopes

A per-call frame's value bindings live in a slot array sized by a per-body layout the body
computes once, not in a hash map the frame builds per activation.

**Problem.** Every per-call scope is born with five empty bump-backed hash tables plus a claim
store ([`Bindings::new`](../../src/machine/core/bindings.rs), reached from
[`child_for_frame_witnessed`](../../src/machine/core/scope.rs)), and each of the body's binds
then pays a hash insert into the `data` map, so an activation of a body whose binding set is
lexically fixed rebuilds the same shape from nothing every time. The shape is knowable up front:
the binders a body declares are already enumerated statically by `statement_binder_plan` /
`binder_name_slot` / `MACHINE_BINDERS` for `CLOSE` inference
([close_inference.rs](../../src/machine/model/close_inference.rs)), but nothing keeps that
enumeration on the body, and the runtime instead stamps each entry's `BindingIndex` from the
submitting statement's `chain.index` at decide time
([submit.rs](../../src/machine/execute/decide/submit.rs)). A name read is a `u128` identity-hash
probe per chain link rather than an index; the "bound reads its table, parked reads the store"
split in the `bindings.rs` module doc exists because a claim and a binding are two structures
keyed on one name; and the copy engine flattens every copied entry's position to
`BindingIndex::value(0)` and re-inserts by name
([copy.rs](../../src/machine/core/scope/copy.rs)), since a copy has no layout to preserve
positions against. The `declare_n10` / `declare_n100` shapes in
[observe/alloc.txt](../../observe/alloc.txt) hold the per-declared-name term this owns.

**Acceptance criteria.**

- A lexical body whose value-binding set is static carries an immutable **layout** — `ValueSymbol
  → slot`, slot count, and each slot's lexical statement position — computed once and homed on
  the body node, per the no-global-runtime-state rule (no run-rooted registry).
- A per-call scope's value bindings are one bump allocation in the frame's region sized by the
  layout, holding one slot state per layout slot; `Bindings::new` builds no `data` map for such a
  scope, and the allocation-baseline `declare_*` shapes rebaseline downward accordingly.
- A slot is `Empty`, `Claimed` on the in-flight binder's `ProducerId`, or `Bound`, so the value
  channel of the claim store is the slot array: a value-name lookup is one slot read answering
  `Bound` / `Parked` / miss, and the copy-readiness gate's "no claims" half is a counter or mask
  read over the array with no store probe.
- Slot contents stay drop-free and `Copy`-cheap to read out; the `needs_drop` assert on
  `Bindings` holds unchanged, and frame death remains O(scopes).
- The positional visibility rule is unchanged: a slot at layout position `i` is visible to a
  reader at cutoff `c` iff `i < c`, with the cutoff still read off the generative `LexicalFrame`
  chain — a static block-id cutoff is not admitted, per the counterexample recorded in
  [frame-recycling.md](frame-recycling.md).
- The copy engine fills a copied per-call scope by an in-order slot walk sharing the source's
  layout verbatim; the `BindingIndex::value(0)` flattening and its justification prose in
  `fill_scope` are gone, and no copied binding is re-inserted by name.
- Scopes outside the item's scope — the run root, module bodies, `SIG` declaration scopes,
  `USING … SCOPE` borrowed windows — keep their keyed tables, and a name read that crosses from a
  slotted frame into a keyed ancestor resolves through the existing façade with no second lookup
  path.
- `functions`, `operators`, and `types` keep their keyed tables in every scope.
- The full slate passes, `tools/seam_equivalence.sh` passes, and the Miri slate is clean.

**Directions.**

- *Which scopes are slotted — decided.* Per-call frame children only: the scopes born through
  `child_for_frame_witnessed` for an `FN` invocation and `EVAL`'s `FreshChild`, each with a fresh
  region brand. Block scopes that share their outer's brand (`child_under`), module bodies, `SIG`
  scopes, USING windows, and the root are out of scope and keep keyed storage.
- *Bodies with a dynamic binding set — decided.* A body the layout cannot fix statically — one a
  metaprogramming splice (`WorkingPart::Spliced`,
  [memory-model.md § Performance notes](../../design/memory-model.md#performance-notes)) rewrites,
  or one whose statements bind through a surface the binder plan does not enumerate — takes no
  layout and keeps the keyed `data` map. The keyed form is the fallback, not a second slotted
  variant with an overflow table.
- *Layout source — decided.* Derived from the same readers `CLOSE` inference sources
  (`statement_binder_plan`, `binder_name_slot`, `MACHINE_BINDERS`, the FN signature stride), so a
  layout and the inferred-capture walk cannot disagree on what a body binds. A second static
  binder enumeration is not admitted.
- *Layout lifetime — open.* (a) bumped into the program-lived AST region beside the body node;
  (b) a heap `Rc` on the body node. Either satisfies no-global-state; (a) matches how a node's
  parts and structural cache are already bumped together at parse
  ([parse/frame.rs](../../src/parse/frame.rs)). Recommended: (a).
- *Types channel — deferred.* `types` could be slotted by the same lexical argument, which would
  delete the residual name-channel claim store entirely, but the `DeclarationSite` installer
  identity and the announced-window interaction need their own check. Left keyed here; a
  follow-up item if the residual store proves to be the next measured term.
- *Slot payload versus the cellgraph carrier — open.* Today a bound entry is a `SealedValue`: the
  value fused to the exact reach description minted for it, stored beside the value
  ([`DataEntry`](../../src/machine/core/bindings.rs)). Under the substrate `workgraph` is being
  rebuilt over ([adopt-cellgraph.md](../../workgraph/roadmap/adopt-cellgraph.md)) that fusion is
  not a legal habitat: a mask may live only in the home cell's reach table, and an at-rest value is
  a `Dormant` carrier naming its reach-table entry by a private key
  ([liveness-matrix.md § Why masks cannot go stale](../../cellgraph/design/liveness-matrix.md#why-masks-cannot-go-stale),
  [dormant.rs](../../cellgraph/src/dormant.rs)). Either the slot's `Bound` payload is defined as
  "the substrate's at-rest carrier" from the start, so adoption changes its type and not the
  array, or it ships as `SealedValue` and adoption rewrites it. Recommended: the former.

## Index-linked chains

A companion idea this item does **not** take on, recorded here so it is weighed against the
shipped substrate rather than rediscovered: replace the `&'a Scope` wiring — `Scope::outer`,
`KFunction::captured_scope` — with indices into an environment table, so a copy of a chain is a
copy of the table with its internal references staying valid unmoved. It attacks the two causes
of copy cost this item leaves alone — identity is an address, and references must be re-anchored
through witnessed doors — where a slot array only removes the position-flattening corner of the
first. The challenges below are open questions, not verdicts; several have a partial answer in
`cellgraph` and are marked so.

1. **An index needs a base.** No-global-state means the table is reached through the value: a
   `KFunction` carries a (table, index) pair rather than one `&Scope`. Under `cellgraph` the base
   is the graph and the index is a `CellHandle`, but what a value stored outside any step holds
   to name the graph, and who owns a chain that spans the per-call tier and the eternal home, is
   unanswered.
2. **Chains span regions.** Today each per-call frame is its own region and eternal-homed scopes
   are referenced verbatim. One indexable table per chain either flattens frames into one region,
   changing the frame-death granularity the O(1) teardown rests on, or is a chain of per-region
   segments with pointers between them. `cellgraph`'s birth relation and tree-cell parent links
   are already the outer chain as indices with per-cell disposal kept
   ([tree-cells.md § Death](../../cellgraph/design/tree-cells.md#death)), so this challenge
   largely dissolves on adoption.
3. **Index soundness has no compile-time checker.** `&'a Scope<'a>` invariance is what proves
   wiring today: a scope cannot name a shorter-lived parent. A raw index is position-independent,
   which is the win, and unchecked, which is the cost. A generation-stamped handle catches a stale
   index at runtime, and that is the tier project policy treats as a stopgap rather than an end
   state; B needs a compile-enforcement story — branded index newtypes, or the carrier types the
   substrate hands out — before it is acceptable.
4. **Out-of-chain references.** Group records (`&'a OperatorGroup`), USING-window borrowed
   tables, announced windows, and sealed carriers inside values all point into regions. B as
   stated re-keys only the scope graph; either these become indices too, into some table, or the
   copy still re-births them and most of `fill_scope` survives.
5. **Sharing and cycles.** The copy memo makes two closures over one scope copy to two closures
   over one copied scope, scope→function→scope cycles included. If each callable carries its own
   (table, index), a copy must still unify callables sharing a chain: either table identity
   replaces the memo or the memo survives keyed by table.
6. **Reach re-derivation is untouched.** Re-sealing each value's reach at the destination is
   semantic. B shrinks wiring fixup, not per-value relocation.
7. **The pin chain.** The `Rc` pin chain (`FrameStorage.outer`) mirrors the scope chain, and the
   eternal tier expresses as a `None` pin. If scopes link by index, what pins the table's segments
   is open on today's substrate; under `cellgraph` birth holds replace the pin chain and eternal
   storage owns no slot, which answers it.
8. **Hot-path indirection.** A chain walk becomes base plus index per link instead of a pointer
   chase — possibly cheaper for contiguity, possibly not for the extra load. Needs a measurement
   on the resolve walk before anything is decided.
9. **Ordering.** Points 1, 2, 3 and 7 all turn on what a reference *is* at the wiring layer, and
   [adopt-cellgraph.md](../../workgraph/roadmap/adopt-cellgraph.md) changes that. B is not to be
   designed before that item's shape is settled; the same soft ordering this item takes, but
   binding rather than soft.

## Dependencies

The liveness matrix the slot array was expected to be co-designed with is prototyped in
[`cellgraph`](../../cellgraph/README.md) and not yet adopted. Reading that prototype against this
item shows the two are on different axes and do not need to be designed as one representation;
the mismatches, and what each means for this item, are recorded here so the co-design question is
not re-opened from scratch:

- **The matrix indexes cells, not bindings.** A row or column of the liveness matrix is a slab
  cell — a frame's region — at a width fixed by the graph's type (`64 · W` slots,
  [§ Pool geometry](../../cellgraph/design/liveness-matrix.md#pool-geometry)); no bit anywhere
  names a variable. The slot array is a per-body axis inside one cell, so it is not "the matrix's
  column space" and the matrix's shape puts no constraint on a layout's slot count. The one
  shareable piece is the inline `Copy` `Bits` row type as a slot-occupancy or claim mask, and its
  width is a type constant while a body's slot count is not.
- **Per-call scopes become tree cells, which hold no bits at all.** The adoption item's kind rule
  makes a per-call frame a [tree cell](../../cellgraph/design/tree-cells.md): no reach table, no
  hold set, and every carrier homed in it reaches `{root}` alone. So the per-slot exact reach a
  `SealedValue` carries today is never consulted by the substrate for a slotted frame, and a slot
  array need not — and cannot — carry per-slot reach for the substrate's benefit.
- **A claim is scheduler state the substrate has no home for.** `Claimed(ProducerId)` names a
  `workgraph` dependency edge; `cellgraph` never decides when a cell runs and has no structure a
  claim could fold into. The claim collapse this item performs is entirely koan-side, and the
  "no claims" readiness read stays an array or mask read rather than a matrix read.
- **A captured frame that seals is read only through the accessor.** A frame that dies with a
  nonzero column seals, and a value read out of a sealed region is re-anchored at the reading
  borrow inside an `enter` scope and hands back no reach
  ([§ The seal transition](../../cellgraph/design/liveness-matrix.md#the-seal-transition)). A
  name lookup that walks into a slotted, sealed captured scope therefore goes through that door,
  so the slot read must not assume it can copy a fused reach out of the array.
- **A tree cell's bump may splice into an ancestor at death.** A slot array bumped into a
  per-call region can outlive the frame as spliced bytes redeemed through tombstones
  ([tree-cells.md § Tombstones](../../cellgraph/design/tree-cells.md#tombstones)). The layout
  must therefore be reachable from the body node, never from the region that held the array —
  which the homing criterion above already requires.
- **Reset-in-place recycling is compatible.** [Frame recycling](frame-recycling.md) resets a
  retiring region's bump and re-mints the child scope through a rebuild door; a layout-sized slot
  array is one bump allocation at rebuild, so the two items compose without either changing shape.

Ordering is soft: this item can ship on today's `workgraph` substrate with the `SealedValue`
payload, at the cost of the payload rewrite the open direction above names when adoption lands.

**Requires:** none — the readers it sources are shipped.

**Unblocks:** none tracked yet.
