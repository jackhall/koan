# Reach ownership split

Splits a value's reach evidence into a non-owning description and a
holder-owned pin bundle. Terms of art are in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary);
the doc of record for the split model is
[design/witness-hosting.md](../../design/witness-hosting.md).

**Problem.** One type does two jobs. A stored reach set is both the
*description* of a value's foreign reach and the *owner* of the strong
`Rc<FrameStorage>` members that pin it
([workgraph/src/witnessed/region_set.rs](../../workgraph/src/witnessed/region_set.rs)),
and the conflation costs on three fronts:

- *Storage.* Owning sets are `Drop`-bearing and live in arena pages, so the
  pages cannot become untyped `Drop`-free bump arenas — the project's end
  state — while sets remain page data.
- *Pin permanence.* Arenas are append-only and there is no un-mint verb, so a
  materialized pin lives as long as the destination region, not the binding
  that wanted it. Pin a call result into a module scope and the dead producer
  frame's region rides the module arena until the module dies. Policy contains
  this by copy-bias: the bind seam
  ([`copy_delivered_substrate`](../../src/machine/core/scope/reach.rs)) never
  pins non-records, and a tail loop's `it` bind is *forced* to copy every
  iteration — pinning would accrete one retired region per hop, destroying the
  O(1) turnover TCO depends on. Half the copy-versus-pin cost model is
  forfeited wherever the destination outlives the producer.
- *Transient ownership.* Two sites use the set type itself as an ownership
  vehicle rather than a description — the run loop's step-open `combined` pin
  ([run_loop.rs](../../src/machine/execute/run_loop.rs)) and
  `check_spliced_return`'s local pin
  ([finalize.rs](../../src/machine/execute/finalize.rs)) — so the ownership
  semantics the split removes are load-bearing there today.

**Acceptance criteria.**

- The reach types split: a non-owning **description** (answers `pins_region` /
  membership queries; keeps nothing alive; `Copy`) and an owned **pin bundle**
  (strong frame-owner `Rc`s, host included). Descriptions live in an
  append-stable side table owned by the region's `FrameStorage`, not in arena
  pages; mint returns the description reference plus the bundle as an owned
  value. Using a description where ownership is required does not compile.
- Every carrier holder owns its bundle or is enveloped under a live one by
  construction: the delivery envelope widens its host `Rc` to the bundle (so
  every envelope duplication carries its pins), binding entries store the
  bundle beside the description, and the transient-pin sites — the run loop's
  step-open pin, the spliced-return check — hold explicit `Rc` bookkeeping.
- Release is ordinary `Drop`: rebind, evacuation, and scope death drop the
  entry's bundle — no release verb, no audit, no new unsafe. A tail loop's
  `it` bind may legally pin: each rebind drops the prior iteration's pins, so
  a pinned loop holds O(1) live regions.
- The `RegionSet<F>: Stored<P>` bounds threaded through the workgraph
  mint/compose/transfer signatures are deleted — the description is no longer
  a storage family.
- The shipped implementation matches
  [design/witness-hosting.md](../../design/witness-hosting.md) — the doc of
  record for the split model (representation, holder rule, retention
  interplay) — with no divergence between its stated invariants and the code.
- The Miri audit slate is green with rebind-release (a dropped pin freeing its
  region) exercised.

**Directions.**

- *Ownership model — decided.* The split above, over an audited release verb
  (a standing runtime hole) and over monotone pins with policy containment
  (forfeits the pin half of the cost model).
- *Staging — open.* Land the storage move (owning sets into the
  `FrameStorage` side table, deleting the `Stored` bounds) as its own commit
  before the ownership inversion, versus one branch with the move as an
  internal checkpoint. Recommended: separate commit — the move is mechanical,
  changes no pin lifetime, and is Miri-checkable on its own.
- *Read path — open.* Whether binding reads hand out the borrowed description
  enveloped under the entry's bundle (no refcount traffic) or clone the
  bundle; cloning only at genuine escape to a new holder is the target.
- *Bundle representation — open.* `Vec<Rc<FrameStorage>>` versus a small-vec
  or single-host fast path; the empty bundle (region-pure value) must stay
  allocation-free.

## Dependencies

**Requires:** none — its one prerequisite, the single escape seam, has shipped.

**Unblocks:**

- [Residence-audit retirement](residence-audit-retirement.md) — holder-owned
  bundles make pin liveness typed, the other half of what subsumes the audit.
- [Region evacuation at frame death](region-evacuation.md) — droppable pins,
  so pricing loop-carried carriers to pin does not chain retired regions
  across a tail loop.
- [Drop-free region death](drop-free-region-death.md) — reach data must leave
  the arena pages (descriptions to the side table, owners to holder bundles)
  before pages can be untyped and `Drop`-free.
