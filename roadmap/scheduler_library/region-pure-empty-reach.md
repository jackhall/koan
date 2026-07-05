# Region-pure values mint the empty reach

Stop folding producer/dep reaches into carriers whose values reach no region —
the fully-owned value's exact reach is empty, and folding anyway is what pins
whole per-call arenas to keep single escaped scalars alive.

**Problem.** The witness layer folds carrier-to-carrier without inspecting the
value, so a value inherits its producer's and deps' full frame reach whether or
not it borrows into them:

- The finalize producer-frame fold
  ([`finalize.rs:67`](../../src/machine/execute/finalize.rs)) reseals **every**
  terminal under `FrameSet::singleton(producer.storage_rc())` — a scalar
  terminal pins its producing frame exactly as a borrowed view does.
- The aggregate-literal fold
  ([`fold_cells`](../../src/machine/execute/dispatch/literal.rs)) unions every
  cell's reach into the aggregate regardless of whether the element reaches
  anything; [`copy_carried`](../../src/machine/execute/lift.rs) deep-clones a
  value into the destination region but the enclosing `transfer_into` carries
  the source witness forward.
- The dep-fold alloc combinators
  ([`StepContext::alloc_with`](../../workgraph/src/witnessed/step_ctx.rs) and
  the `alloc_*_with` wrappers in
  [`arena.rs`](../../src/machine/core/arena.rs)) union every named dep's reach
  into the built value's witness, even when the built value embeds no dep
  borrow.
- Exactly one site applies the empty-reach shortcut:
  [`scalar_key`](../../src/machine/execute/dispatch/literal.rs) reads a dict
  key out in place because a key is a scalar reaching no region. The same fact
  is never used for values.

The bounding mechanisms (home-omission at bind, co-lineal subsumption,
transient node reclaim) cover intermediates and per-call bindings; the
residual retention is:

- **Top-level bindings** (`LET x = (f 1)` at run scope): the run frame does not
  pin the callee frame `F`, so home-omission cannot drop it; the binding's
  stored reach keeps `{F}` — the entire per-call arena — for the program's
  life even when `x` is a scalar copied into the run region.
- **Aggregates and folds over cross-lineage producers**: N sibling producer
  frames are neither co-lineal (subsumption cannot collapse them) nor
  ancestors (home-omission cannot drop them), so all N stay pinned as long as
  the aggregate lives — O(N) in a runtime quantity such as element count.

Direction is safe (over-retain, never dangle): this is footprint, not
correctness. The Miri slate is clean; no test pins the retention behavior in
either direction.

**Acceptance criteria.**

- A terminal whose value is fully owned (a scalar `KObject` variant, or a deep
  copy that produced no region borrow) is sealed with the **empty** reach: the
  finalize fold does not union the producer frame into it.
- An aggregate literal's witness unions only the reaches of cells whose values
  reach a region; fully-owned elements contribute no members.
- The dep-fold alloc combinators produce an empty-reach carrier when the built
  value is fully owned, and the uniform carrier-to-carrier fold is unchanged
  for region-reaching values (closures, modules, borrowed views).
- A test binds a scalar result of a user-fn call at run scope and observes the
  callee's `FrameStorage` dropping at call end (e.g. via an `Rc` count or
  arena-retention probe) rather than surviving to program end.
- A before/after retention measurement on a fold-heavy program exists in the
  test suite or `observe/`, showing the sibling-producer frames of a
  scalar-element aggregate are released.
- The full Miri audit slate passes: 0 leaks, 0 UB.

**Directions.**

- *Fold points — decided.* The three fold families above (finalize reseal,
  aggregate cell fold, dep-fold combinators) are the exhaustive set of sites
  to gate; the gate asks "is this value fully owned?" at the fold point and
  skips the union when yes.
- *Purity detection — open.* (a) Shallow, conservative `KObject` variant check
  at the fold point: only variants that structurally cannot hold a region
  borrow qualify (the `scalar_key` rule generalized); (b) a memoized
  region-purity bit carried on the value, analogous to the memoized carried
  `KType` dispatch trusts. Recommended: (a) first — it is decidable locally,
  errs only toward today's over-retention, and (b) can layer on later if
  shallow misses matter.
- *Copy-purity at `transfer_into` — open.* A deep clone whose result embeds no
  region borrow could reseal under the destination-only witness rather than
  destination ∪ retained sources. (a) Detect via the same purity check on the
  copied result; (b) have `copy_carried` report whether it retained any borrow.
  Recommended: (b) — the copy already walks the value, so it knows.

## Dependencies

Soft ordering with
[region-hosted-witness-sets](region-hosted-witness-sets.md): hosting extends a
set's life to its region's death, so mint-time precision does more work there
— land this with or before it. Neither hard-requires the other.

**Requires:** none — implementable against the current fold sites.

**Unblocks:** none tracked — a leaf footprint item.
