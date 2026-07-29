# Value substrates and escape policy

Pins the target storage model for composite runtime values: every composite
value's substrate is region-allocated and carried as a plain borrow, regions
tend toward Drop-free untyped arenas, and an escaping value transfers by
pinning its birth region — with a cost-driven copy as a pure optimization.
One ownership regime, no per-value refcounts.

## Vocabulary

Terms this doc and the [untyped_arena](../roadmap/untyped_arena/README.md)
roadmap items use with fixed meanings. The machinery behind them is owned by
[per-node-memory.md](per-node-memory.md) (the witnessed substrate) and
[memory-model.md](memory-model.md) (the region/frame protocol); this list is
just enough to read the policy.

- **Region** — the per-call allocation unit
  ([`KoanRegion`](../src/machine/core/arena.rs)): a set of arenas owned by one
  call frame, freed all at once when the frame's last hold drops. "Arena"
  names storage inside a region.
- **Substrate** — the stored cells behind a composite value (a list's element
  slice, a record's field record, a dict's frozen map). The **value carrier**
  is the [`KObject`](../src/machine/model/values/kobject.rs) enum value that
  borrows the substrate and carries the memoized `KType` beside it.
- **Brand** — a rank-2 (`for<'b>`) lifetime naming one specific region inside
  a closure's scope, so "this allocation went into that region" is a
  compile-time fact rather than a runtime check
  ([per-node-memory.md](per-node-memory.md)).
- **Door** — a construction entry point holding a region's brand; the only way
  a composite substrate gets built. Allocating through a door is what makes
  the residence question compile-enforced.
- **Witness** — held liveness evidence (`Rc<FrameStorage>` holds) proving
  every region a value borrows from is still alive; the combinator enclosing a
  door composes it from the operands' own witnesses
  ([per-node-memory.md](per-node-memory.md)).
- **Reach** — the set of foreign regions a value's borrows can point into.
  **Minting** a reach produces a paired non-owning description (into the
  consumer region's reach table) and owned pin bundle (stored by the
  consuming holder), so the consumer keeps the regions alive with no help
  from the producer ([memory-model.md § Region lifetime erasure](memory-model.md#region-lifetime-erasure)).
  Minting is get-or-mint against the region's interned description table
  ([workgraph/design/sectioned-reach.md](../workgraph/design/sectioned-reach.md)).
- **Pin** — keeping a producer's whole region alive by holding its
  `Rc<FrameStorage>` in the consuming holder's bundle, released when that
  holder drops; the escape default below.
- **Seam** — the one relocation choke-point every region crossing routes: the
  [`transfer_into`](../workgraph/src/witnessed/delivered.rs) fold and its
  [`copy_carried`](../src/machine/execute/lift.rs) hook.
- **Drop-free** — a stored (`'static`) form that owns no heap data: dropping
  it is a no-op, so its bytes can be reclaimed without running any destructor.

## One ownership regime

Every composite [`KObject`](../src/machine/model/values/kobject.rs) payload is a
**region-allocated substrate**, borrowed by the value carrier:

- `List(&'a ListSubstrate<'a>, KType)` — the element slice in the arena.
- `Dict(&'a DictSubstrate<'a>, KType)` — an arena-frozen immutable map (layout
  free: a sorted-pair slice or a hash table frozen at construction).
- `Record(&'a RecordSubstrate<'a>, KType)` — the field record in the arena.
- `Tagged { value: &'a PayloadSubstrate<'a>, .. }` — the single-cell payload
  substrate (`PayloadSubstrate = ContainerSubstrate<KObject>`) in the arena.
- `Wrapped { inner: &'a PayloadSubstrate<'a>, .. }` — the same one-cell payload
  substrate; the peel (re-tag collapses one layer) and hold (construction
  preserves layers) constructors are door verbs, not a payload wrapper type.
- `KFunction(&'a KFunction<'a>)` and `Module(&'a Module<'a>)` — bare borrows
  into their defining regions.
- Scalars (`Number`, `Bool`, `Null`) are owned leaves. `KString` rides an
  arena `&'a str`, and `KExpression`'s part vectors are arena slices
  ([§ Untyped arenas](#untyped-arenas-the-drop-free-end-state)).

Each cell-bearing substrate is one payload-generic **wrapper struct**,
[`ContainerSubstrate<C>`](../src/machine/model/values/container_substrate.rs):
the stored cells (`C`) beside a single `SubstrateMemos` value bundling the
substrate memos of [§ Construction](#construction-witnessed-doors-only) — the
copy cost, the contains-borrows bit, and the borrows-home bit ride the
substrate; the type handle rides the value carrier. The per-container names
above are aliases of that one wrapper: `RecordSubstrate` is
`ContainerSubstrate<Record<Held>>`, and each later conversion instantiates the
same wrapper over its own payload (`ContainerSubstrate<Vec<Held>>` for a list,
a frozen map for a dict, `ContainerSubstrate<KObject>` — the `PayloadSubstrate`
alias — for the single payload cell a `Tagged` or `Wrapped` value borrows)
rather than re-deriving a parallel struct and memo trio.

Three consequences define the regime:

- **Values never move.** A substrate lives where it was born for the life of
  its region. `deep_clone` is a pointer copy for every composite arm.
- **Substrates are immutable after construction.** There are no interior field
  writes anywhere in the runtime; every consumer reads. The retype path
  (`stamp_type`, the `*_with_type` constructors, the FROM narrowing
  projection) shares the substrate borrow and swaps the memoized `KType` —
  it never touches cells.
- **No second ownership channel.** No composite payload rides an `Rc`, so no
  value's clone bumps a refcount and no value's drop runs payload `Drop` glue.
  Sharing happens at exactly one granularity: the region
  (`Rc<FrameStorage>`, held by frames and pin bundles).

## Construction: witnessed doors only

Every composite substrate is born through a **branded door** — a fold placement
([`FoldedPlacement`](../workgraph/src/witnessed.rs) via
[`FoldingBrand`](../src/machine/core/arena.rs)), the step allocator, or a
scope door — whose enclosing combinator composes the witness naming every
operand the value was built from. Residence is
compile-enforced by the door's brand: there is **no runtime residence audit
and no structural residence walk** for composite values. The rank-2 brand
discipline that makes this sound is the substrate contract in
[per-node-memory.md](per-node-memory.md).

Region-free value construction exists only for shapes that own their data
outright (scalars, quoted expressions); no container is ever built without a
door in hand. Construction memoizes, in one pass over the cells (the same
pass that computes the type join):

- the value's own **type handle** (the existing memo, [typing/type-registry.md](typing/type-registry.md));
- its **copy cost** — see [§ Cost-driven copy](#cost-driven-copy-the-optimization);
- a **contains-borrows bit** — whether any transitive cell is a region-borrow
  leaf (a closure or module), into *any* region;
- a **borrows-home bit** — whether any transitive borrow leaf points into the
  substrate's *own home region*. The exact, home-relative gate the cost decision
  reads (see [§ Cost-driven copy](#cost-driven-copy-the-optimization)), distinct
  from the conservative contains-borrows bit above.

The two borrow bits are folds over the substrate's run descriptions
([§ Sectioned reach](#sectioned-reach)), not a separate walk.

## Sectioned reach

Reach is a stored fact at every granularity, not just per value. A
substrate's cells are physically partitioned into contiguous **runs** in
semantic order (field order, list index order, dict iteration order), each
run pairing a span of cells with one reach description interned in the home
region's side table. The run machinery — the interned table, the
payload-generic sectioned storage, the alloc door that groups enveloped
inputs into runs and folds their pins — is workgraph's
([workgraph/design/sectioned-reach.md](../workgraph/design/sectioned-reach.md));
koan supplies the per-input copy-or-pin verdicts and the born-borrowing
seeds (the `FN` door naming a closure's captured scope, the module door
naming its child scope).

- **Exact per run.** A run's description is precisely the shared reach of
  its cells, so a projection or index read hands a cell out with exactly
  its own reach: the bind or lift seam mints from the stored description —
  upgrade to a bundle under the container's covering pin, then one mint
  source — never a subset walk over the container. A cell is `'a`-confined
  to its container's region until such a seam relocates it; the compiler
  forces the seam.
- **Owned-only channels.** Scalars, strings, and type values are owned data
  (a koan type value never embeds a value), so they always land in
  empty-reach runs. **Dict keys are restricted to owned data** by language
  rule — a function or module key is meaningless — and the construction
  door rejects a borrow-carrying key by its stored envelope, an O(1) check.
- **Memos are folds over runs.** Contains-borrows ⇔ any run's description
  is non-empty; borrows-home ⇔ any run names the home region; `copy_cost`
  is unchanged.
- **Transfer reads stored reach.** A crossing claims the empty source
  bundle exactly when no surviving run names the source region — a stored
  fact, not a probe over the value's shape.
- **The value-level description stays.** Whole-value carriers (delivery
  envelopes, binding tables) keep their single stored description — the
  get-or-mint of the union over the value's runs, cheap under interning.

## Escape: pin by default

An escaping value — a return, an argument bind, a root-drain terminal —
**keeps its borrows and pins its birth region**. The consumer takes the
producer's frame-retention hold (`Rc<FrameStorage>`) and mints the value's
reach pair against its own scope — description into the reach table, pins
onto the binding entry — the same protocol every closure and module already
rides ([memory-model.md § Region lifetime erasure](memory-model.md#region-lifetime-erasure)).
Transferring ownership of an arbitrarily large container is therefore one
refcount bump and one reach mint: **O(1), zero bytes moved**, at region
granularity.

The price of the pin is retention granularity: the consumer retains the whole
producer region — the result *and* the call's temporaries — until the
pinning entry drops (rebind, evacuation, or scope death). The copy
optimization below exists to bound exactly that cost.

## Cost-driven copy: the optimization

At the one relocation seam every crossing routes (the
[`transfer_into`](../workgraph/src/witnessed/delivered.rs) fold and its
[`copy_carried`](../src/machine/execute/lift.rs) hook — consumer pulls,
forward pulls, seed binds, the root drain), the runtime chooses per value:

- **Copy** — rebuild the value's entire reachable structure at the destination
  brand, releasing the producer pin. Cells that are region-borrow leaves
  (closures, modules) ride as borrows in either verb; their own reaches ride
  the witness unchanged. A copy is total or not at all — a partial spine copy
  would pay the copy *and* keep the pin.
- **Pin** — the default above: borrow rides, region transfers by hold.

The core decision is a **scale-free ratio** over two numbers that already exist
at the seam:

- **`copy_cost`** — memoized on every substrate at construction: leaves
  contribute their weight (cell count as the first cut; byte-weighted where a
  leaf's size varies, a string being the motivating case), nested substrates
  contribute their own memoized cost, borrow leaves contribute zero. A cell that
  is a spliced expression is **unpriceable**: it carries no memo of its own, so
  the whole substrate's cost saturates to a sentinel and the value copies
  unconditionally (releasing per the exact probe below). Because substrates are immutable
  the memo can never go stale, and because the copy verb rebuilds a shared
  subvalue once per reference, a priceable memoized sum is the copy's *exact*
  cost — no forwarding map, no walk.
- **the region's allocated total** — its arenas already know their size.

For a priceable value crossing out of its **own home region**, the rule is that
ratio: copy when `copy_cost < α × region_allocated` — "this value is a small
fraction of what the pin would retain." A value that is most of its region pins
(retention barely exceeds the value; the copy would be pure CPU); a small result
escaping a fat frame copies and releases it. α is a tuning constant of the seam,
not observable in language semantics. A **foreign crossing** — the value is
resident in a region the producer host does not own — always pins: pricing a
copy-out at an intermediate host is region evacuation's job, not the
per-crossing seam's.

The ratio is gated by the exact **borrows-home bit**. Set, the value **pins
outright** — a leaf provably borrows the home region, so a copy would pay the
rebuild *and* keep the pin; the ratio is never consulted. Clear on a priceable
value, the copy provably releases the host (no surviving borrow reaches it), so
the ratio alone decides. This is why borrows-home is a *separate*, sharper memo
than the conservative contains-borrows bit: contains-borrows asks only whether
*any* borrow leaf exists into *any* region, and remains the seal/reach
conservatism input; the copy decision needs the home-relative question, and gets
an exact answer for a priceable value. On the **unpriceable** path, where no
home-relative memo is available, release is still a stored fact: the copy claims
the retiring host's release exactly when no surviving run description names it
([§ Sectioned reach](#sectioned-reach)), so a value whose leaves all point into
foreign regions still releases its home.

A **pinned record** shares its producer-resident substrate by a pointer-copy
(never a partial rebuild). Because a record's substrate borrow carries no borrow
naming its *own* home region, the bind seam names the producer region
**explicitly** in the pinned value's reach — rather than leaning on ambient
coverage the way a closure's captured region does — so the residence audit can
evidence the shared substrate through a reach-description member. The explicit naming is
redundant-but-harmless: the producer region is already ambiently rooted for the
binding's life.

The policy is **semantically invisible**: koan values are immutable and
identity-free, so nothing in the language can distinguish a copied result from a
pinned one. Two mutually-exclusive build features (`seam-force-copy`,
`seam-force-pin`) force every record escape seam to a single verb, turning the
whole output-asserting suite into an **equivalence battery** — identical
hardcoded expectations passing under both prove the choice changes only which
memory mechanism runs, never observable behavior. This is also the seam where
**region evacuation** becomes a local decision: at frame death with escapees,
the same two numbers price copying-the-survivors-out against
transferring-the-region.

## Untyped arenas: the Drop-free end state

A **storage family** is one stored type's sub-arena inside a region. Families
split by one rule: a family whose stored (`'static`) form is **Drop-free moves
into a shared untyped bump arena** — untyped meaning the arena tracks only
bytes and alignment, with no per-slot type or destructor bookkeeping, which is
exactly what Drop-freedom licenses. Region death for those bytes is
deallocation with no per-slot `Drop` glue: free the arena's chunks, done.
Families whose slots own heap data stay in typed sub-arenas until converted,
and the families that are *designed* to own things — a `FrameSet`'s region
holds — remain typed and droppy permanently; "as much storage as possible"
means the value substrates.

A scope's binding tables are **not** in that residue. An entry is a
`BindingIndex` beside a resting `Sealed` carrier, both `Copy` and `Drop`-free:
the pins keeping the entry's reach alive live one level down, in the region's own
union bundle, which drops whole at region death
([witness-hosting.md § The pin bundle](witness-hosting.md#the-pin-bundle)).

## Invariants preserved

- **Cycle-freedom needs no gate.** No stored value owns an `Rc` back to any
  region — a substrate borrow is a borrow, a reach's pins are holder-owned
  and the self and eternal rules keep ownership acyclic
  ([witness-hosting.md § Composition](witness-hosting.md#composition-minting-a-description-and-retaining-its-pins))
  — so the allocation engine keeps
  needing no cycle gate ([memory-model.md](memory-model.md)).
- **Directionality.** Inward references stay free; outward references exist
  only on the escape path and are always covered by a minted reach. The
  residence *question* ("does this value borrow outside its region?") is
  answered at construction by the door's brand, never re-derived by a walk.
- **Verification.** The Miri audit slate ([observe/miri_slate.md](../observe/miri_slate.md))
  remains the sign-off gate: zero UB, zero process-exit leaks across the
  slate, with the escape seam's copy and pin verbs both exercised.

## Open work

The [untyped_arena](../roadmap/untyped_arena/README.md) roadmap project carries
the conversion slate; its `Requires` chain encodes the order:

- [Residence-audit retirement](../roadmap/untyped_arena/residence-audit-retirement.md)
- [Region evacuation at frame death](../roadmap/untyped_arena/region-evacuation.md)
- [Region-store string values](../roadmap/untyped_arena/region-store-strings.md)
- [Region-store expression parts](../roadmap/untyped_arena/region-store-expressions.md)
- [Drop-free region death](../roadmap/untyped_arena/drop-free-region-death.md)
