# Region bump storage for embedder value families

Expands the region's public surface with the bump door an embedder needs to host
a `Drop`-free value family as bytes, alongside the typed family cells.

**Problem.** A [`Region`](../src/witnessed/region.rs)'s only bump surface is
`alloc_side`: `pub(crate)`, slice-only, `T: Copy`, and documented as the home for a
`Sectioned` container's run partition and cell index block. Every public allocation
path on `RegionHandle` — `alloc`, `alloc_resident`, `alloc_resident_checked` — reaches
only the typed `FamilyArena` cells, whose `Arena<K::At<'static>>` slot type routes the
store through `erase_to_static` and leaves a value that holds an `&'a` back into its own
region no door but `alloc_resident_checked` and an `AuditedStored` runtime residence
audit. So an embedder converting a `Drop`-free, region-self-referential family has no
way to reach the byte arena that already exists one field over, and pays a runtime audit
for storage that needs no erasure at all.

**Acceptance criteria.**

- **One** public bump door, generic over the family it stores, taking its operands **as
  carriers** and a rank-2 `for<'b>` constructor that builds the value in place from their
  opened views — the byte-storage analogue of the fold engine's rank-2 placement door
  ([`FoldedPlacement`](../src/witnessed.rs)), whose enclosing combinator already composes
  the witness naming every operand its closure built from.
- The door names **no workload type**, holding the invariant `region.rs` already states
  for the storage substrate. The constructor writes through a bump placement whose
  primitives are std shapes only — a `Copy` value, a `Copy` slice, a `str`. There is no
  per-family verb, so an embedder converting a new family reaches for the same door rather
  than the library growing a door per workload shape.
- The engine opens the operands, composes their witness, mints and retains the reach, and
  hands back one **bundled carrier**. The embedder names no `StepCoverage`, no
  `ReachDescription` and no `PinBundle` at any point: reach is a consequence of which
  carriers were passed in, so it is neither derivable nor forgeable at the call site.
- A door call with no operands yields empty reach structurally — there is no coverage
  claim for a call site to write, correctly or otherwise.
- The constructor's bare `&'b` views cannot escape: `'b` has no outlives relation to any
  enclosing lifetime, so a capture is a compile error, exactly as it is at
  `FoldedPlacement::alloc_resident_folded`. Bare inside the brand, bundled on the way out.
- No entry point returns a bare region reference, and none returns a `(value, reach)` pair.
- The door carries no `unsafe` and no lifetime erasure: a value stored through it may
  hold an `&'a` into the same region with no `AuditedStored` impl and no residence audit.
- Every placement primitive keeps the `T: Copy` bound — the static proxy for "no destructor
  to skip", which the bump never runs.
- `Region` reports its bump occupancy as total live bytes through a public reader.
- [witnessed-memory.md](../design/witnessed-memory.md) and the `region.rs` doc comments
  name the bump the storage home for embedder value families, not only `Sectioned` side
  data.
- The library slate is green: `cargo test -p workgraph` and
  `cargo +nightly miri test -p workgraph --lib`.

**Directions.**

- *Per-family byte attribution — decided.* Not reported. One shared per-region bump,
  the same one `alloc_side` writes to; total live bytes is the only occupancy figure
  anyone consumes, since the copy-versus-pin decision reads the region's total against
  the candidate value's own copy size and never needs a family breakdown.
- *Widening past `Copy` — deferred.* "`Drop`-free" has no expressible bound; a
  `const { assert!(!mem::needs_drop::<T>()) }` gate is the honest widening if a family
  ever needs one. Every family queued behind this door is `Copy` by construction, so
  nothing needs it yet.
- *Which carrier family the door hands back — decided.* `Opened<'b, _, _>`. The product is
  consumed inside the enclosing brand — bytes becoming a value's field, a part slice
  becoming an expression — so confinement must be a compile error, and `Opened` is the
  state that carries the lifetime. `Witnessed` erases it, and `Sealed` is the resting form,
  which would mean erasing a borrow the caller is about to consume.
- *How a door-born `Opened` reaches the resting form — open.* `Opened` is today
  constructible only by opening a seal or delivery — which is what makes its value↔reach
  pairing unforgeable — and `reseal` returns exactly the seal the value came from. A door
  product has no prior seal. Options: give the door its own `Opened` constructor (engine-only,
  so the pairing stays unforgeable because the engine minted the reach) with `reseal`
  minting a fresh seal; or leave the door's product brand-confined with no resting path at
  all, on the grounds that a bump-hosted operand always ends up inside a larger value that
  has its own. The second is narrower and worth trying first — every family queued behind
  this door is an operand, not a slot terminal.
- *Whether the door is one verb or several — decided.* One, generic over the stored family.
  A bytes-only allocation (a string literal, a keyword slice) is the same verb with an
  empty operand list, not a second verb: its empty reach already falls out of having no
  operands. Per-shape verbs would mean koan defining a door per family, which is the
  library growing workload knowledge it is built not to have.
- *Whether `alloc_side` survives as a separate verb — decided.* Yes, crate-private, for
  `Sectioned` side data. It is not a duplicate spelling of the public door: its caller is
  `Sectioned::build`, which mints the container's description in the same call and hands
  the pair up under the door discipline, so the bare slice never leaves the library.

## Dependencies

Purely additive to workgraph's surface, so it lands without touching koan — the migrate
side is the three items under Unblocks.

**Requires:** none — foundation; the per-region `Bump` already exists.

**Unblocks:**

- [Region-store string values](../../roadmap/untyped_arena/region-store-strings.md)
- [Region-store expression parts](../../roadmap/untyped_arena/region-store-expressions.md)
- [Region-hosted operator groups](../../roadmap/untyped_arena/region-hosted-operator-groups.md)
