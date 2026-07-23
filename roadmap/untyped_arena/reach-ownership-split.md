# Reach ownership split and the single escape seam

Splits a value's reach evidence into a non-owning description and a
holder-owned pin bundle, and makes the bind seam the only escape mechanism —
declared returns re-stamp in place instead of relocating. Terms of art are in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary);
the representation this replaces is
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
- *Escape duality and host drift.* Declared returns escape at the Done
  boundary — [`finalize_terminal`](../../src/machine/execute/finalize.rs)
  relocates the value into the contract's home region — while undeclared
  returns escape at the bind seam. The relocation channel is where a delivery
  envelope's host drifts from the value's residence: after the transfer the
  scheduler re-pairs the *producer frame's* retention hold as host, the
  carrier's `borrows_host` referent silently names a region the value no
  longer borrows, and the runtime residence audit
  ([`Residence::owns_substrate`](../../src/machine/core/arena/residence.rs))
  spuriously rejects valid programs — chaining a substrate-returning operator
  three deep through a binding

  ```
  LET r1 = {a = 1}  LET r2 = {a = 2}  LET r3 = {a = 3}
  MODULE recs = ((OP #(&) OVER :{a :Number} = (right)) (LET chained = (r1 & r2 & r3)))
  ```

  errors with `borrows a region not covered by dest, the supplied evidence, or
  the destination scope's ambient coverage`. Three sites additionally use the
  set type itself as a transient ownership vehicle rather than a description —
  [`ReturnObligation::seal`](../../src/machine/execute/obligation.rs)'s
  self-pinning singleton, the run loop's step-open `combined` pin
  ([run_loop.rs](../../src/machine/execute/run_loop.rs)), and
  `check_spliced_return`'s local pin — so the ownership semantics the redesign
  removes are load-bearing there today.

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
  step-open pin, the spliced-return check — hold explicit `Rc` bookkeeping
  (the obligation cell needs none; see below).
- Release is ordinary `Drop`: rebind, evacuation, and scope death drop the
  entry's bundle — no release verb, no audit, no new unsafe. A tail loop's
  `it` bind may legally pin: each rebind drops the prior iteration's pins, so
  a pinned loop holds O(1) live regions.
- One escape mechanism: the object channel re-stamps a declared return in
  place in the producer's region; the Done-boundary relocation channel is
  deleted. `ReturnObligation` seals to pure `Copy` data — the declared-type
  registry handle plus the precomputed label; the self-pinning home-owner
  witness is deleted along with its relocation-destination role — and the
  keep-first check still fires once, at the end of a tail chain.
- The pairing "a delivery envelope's host pins its value's residence region"
  holds by construction — with no relocation, host, producer pin, and
  residence coincide, and an envelope stating otherwise has no constructor.
- The chained-operator program above evaluates without a residence-audit
  rejection.
- The runtime residence audit is retired where the typed pairing subsumes it,
  or documented as a redundant backstop.
- The `RegionSet<F>: Stored<P>` bounds threaded through the workgraph
  mint/compose/transfer signatures are deleted — the description is no longer
  a storage family.
- The shipped implementation matches
  [design/witness-hosting.md](../../design/witness-hosting.md) — the doc of
  record for the split model (representation, holder rule, retention
  interplay) — with no divergence between its stated invariants and the code.
- The Miri audit slate is green with rebind-release (a dropped pin freeing its
  region) and re-stamp-in-place declared returns both exercised.

**Directions.**

- *Ownership model — decided.* The split above, over an audited release verb
  (a standing runtime hole) and over monotone pins with policy containment
  (forfeits the pin half of the cost model).
- *Escape default — decided.* Re-stamp in place; every escape resolves at the
  bind seam. Cost accepted: a pinned return's residence is the producer
  frame's whole region, garbage included — the compaction countermeasure is
  [region evacuation](region-evacuation.md), which composes with droppable
  pins rather than replacing them.
- *Read path — open.* Whether binding reads hand out the borrowed description
  enveloped under the entry's bundle (no refcount traffic) or clone the
  bundle; cloning only at genuine escape to a new holder is the target.
- *Bundle representation — open.* `Vec<Rc<FrameStorage>>` versus a small-vec
  or single-host fast path; the empty bundle (region-pure value) must stay
  allocation-free.

## Dependencies

**Requires:** none — a redesign of existing carrier machinery.

**Unblocks:**

- [Region evacuation at frame death](region-evacuation.md) — the all-carrier
  bind-seam pricing needs drift-free residence and droppable pins, so pricing
  every carrier neither hits spurious residence rejects nor chains retired
  regions across a tail loop.
- [Drop-free region death](drop-free-region-death.md) — reach data must leave
  the arena pages (descriptions to the side table, owners to holder bundles)
  before pages can be untyped and `Drop`-free.
