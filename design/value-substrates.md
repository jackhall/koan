# Value substrates and escape policy

Pins the target storage model for composite runtime values: every composite
value's substrate is region-allocated and carried as a plain borrow, regions
tend toward Drop-free untyped arenas, and an escaping value transfers by
pinning its birth region — with a cost-driven copy as a pure optimization.
One ownership regime, no per-value refcounts.

## Vocabulary

Terms this doc and the [untyped_arena](../roadmap/untyped_arena/README.md)
roadmap items use with fixed meanings. The machinery behind them is owned by
[workgraph/design/witnessed-memory.md](../workgraph/design/witnessed-memory.md)
(the witnessed substrate) and
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
  ([witnessed-memory.md](../workgraph/design/witnessed-memory.md)).
- **Door** — a construction entry point holding a region's brand; the only way
  a composite substrate gets built. Allocating through a door is what makes
  the residence question compile-enforced.
- **Witness** — held liveness evidence (`Rc<FrameStorage>` holds) proving
  every region a value borrows from is still alive; the combinator enclosing a
  door composes it from the operands' own witnesses
  ([witnessed-memory.md](../workgraph/design/witnessed-memory.md)).
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
  substrate in the arena.
- `Wrapped { inner: &'a PayloadSubstrate<'a>, .. }` — the same one-cell payload
  substrate; the peel (re-tag collapses one layer) and hold (construction
  preserves layers) constructors are door verbs, not a payload wrapper type.
- `KFunction(&'a KFunction<'a>)` and `Module(&'a Module<'a>)` — bare borrows
  into their defining regions.
- Scalars (`Number`, `Bool`, `Null`) are owned leaves. `KString` rides a
  bump-hosted `&'a str` ([§ String residence](#string-residence)), as does a
  `Tagged` discriminant and a `KKey::String` dict key.
  [`KExpression`](../src/machine/model/ast.rs) is a `Copy` handle whose parts run
  is a bumped slice of `Copy` parts
  ([§ Untyped arenas](#untyped-arenas-the-drop-free-end-state)).

Each cell-bearing substrate is one index-generic **wrapper struct**,
[`ContainerSubstrate<'a, C>`](../src/machine/model/values/container_substrate.rs):
the cells in workgraph's sectioned storage ([§ Sectioned reach](#sectioned-reach)),
a payload-specific index `C` mapping a name / key / position onto a cell index,
the interned union over the runs, and the memoized copy cost. Reach and cost ride
the substrate; the type handle rides the value carrier. The per-container names
above are aliases of that one wrapper, differing only in `C`: `Record<usize>`
for a record's name→index table, a marker type for a list (position *is* the
index) and for the single-cell payload a `Tagged` or `Wrapped` value borrows, a
frozen `HashMap<KKey, usize>` for a dict. Each later conversion instantiates the
same wrapper over its own index rather than re-deriving a parallel struct.

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
[witnessed-memory.md](../workgraph/design/witnessed-memory.md).

Region-free value construction exists only for shapes that need no door — the
scalars, which own their data outright, and a quoted expression, whose parts run
already sits in the eternal-tier storage that parsed it; no container is ever
built without a door in hand. The door is a brand paired with the **holder-rule proof** its
per-cell verdicts are read under
([`SubstrateDoor`](../src/machine/core/arena.rs)), so a container cannot be
built through a bare brand: a cell that keeps borrowing a foreign source hands
the alloc door that source's stored description, and reading a description's
members back out is sound only while something pins every region it names.
There is no holderless door — a site whose cells are all owned data names the
empty coverage explicitly, so "nothing to prove here" is a claim written at the
call site rather than a shape a caller can fall into.

Construction memoizes, in one pass over the cells (the same pass that computes
the type join):

- the value's own **type handle** (the existing memo, [typing/type-registry.md](typing/type-registry.md));
- its **copy cost** — see [§ Cost-driven copy](#cost-driven-copy-the-optimization);
- the value-level **reach description** — the interned union over the cells'
  runs ([§ Sectioned reach](#sectioned-reach)), which the alloc door mints.

The two borrow facts a seam reads are queries on that stored union, not separate
bits: **contains-borrows** — whether any transitive cell is a region-borrow leaf,
into *any* region — is "the union is non-empty"; **borrows-home** — whether any
transitive borrow leaf points into the substrate's *own home region* — is the
description's own `members ∋ host` query, the exact home-relative gate the cost
decision reads (see [§ Cost-driven copy](#cost-driven-copy-the-optimization)) and
sharper than the conservative contains-borrows question.

Per cell, the door's verdict is one O(1) read of stored facts and never a walk:
owned data (a scalar, a string, a type value, a quoted expression)
lands in an empty-reach run; a nested substrate hands in its own stored union;
a closure or module is a born-borrowing seed naming the scope it borrows.

A seed must also name where the leaf **lives**, and the two coincide for only
one of them. A closure is allocated into the very region that owns its captured
scope — release-enforced at the allocation door — so naming the scope names the
residence too. A module carries no such invariant: transparent ascription
re-tags a foreign module into the viewing scope's own region, so its residence
and its child scope's region genuinely differ and the residence is not
recoverable from the value. A module seed therefore names its child scope *and*
the door's holder, which pins wherever the cell was read from.

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
naming its child scope and its residence — see
[§ Construction](#construction-witnessed-doors-only) for why only the module
needs both).

- **Exact per run.** A run's description is precisely the shared reach of
  its cells, so a projection or index read hands a cell out with exactly
  its own reach: the bind or lift seam mints from the stored description —
  upgrade to a bundle under the container's covering pin, then one mint
  source — never a subset walk over the container. A cell is `'a`-confined
  to its container's region until such a seam relocates it; the compiler
  forces the seam.
- **Owned-only channels.** Scalars and type values are owned data (a koan
  type value never embeds a value), so they always land in empty-reach runs.
  A **string** lands in one too, by a different route: its bytes are a
  region borrow, so the door re-bumps them into its own region before the
  verdict is read ([§ String residence](#string-residence)) and the cell then
  borrows nothing outside the container it is landing in. **Dict keys are
  restricted to owned data** by language rule — a function or module key is
  meaningless — enforced at the one site that turns a carrier into a key on
  the dict door's own construction path
  ([`scalar_key`](../src/machine/execute/dispatch/literal.rs)), which rejects
  a carrier naming any reach member by its stored envelope: an O(1) check,
  not a walk. `KKey` then admits only `String` / `Number` / `Bool`, so a key
  naming a substrate or a closure is unrepresentable downstream of that site,
  and the one borrow a key does carry — a string's bytes — is re-bumped into
  the dict's own region as the key table freezes.
- **Memos are folds over runs.** Contains-borrows ⇔ any run's description
  is non-empty; borrows-home ⇔ any run names the home region; `copy_cost`
  is unchanged. Borrows-home is exact because of the **run-level self rule**
  at the alloc door
  ([sectioned-reach.md § The alloc door](../workgraph/design/sectioned-reach.md#the-alloc-door)):
  a cell already resident in the destination contributes no residence member
  of its own, so a set bit means a genuine borrow *leaf* points home rather
  than merely that the container holds a co-resident sub-container.
- **Transfer reads stored reach.** A crossing claims the empty source
  bundle exactly when no surviving run names the source region — a stored
  fact, not a probe over the value's shape. A **bare borrow leaf** has no
  runs to read and is never rebuilt (every copying relocation carries its
  reference verbatim), so it keeps the region it lives in unconditionally:
  the only region a relocation may release is the value's own home, and a
  leaf still borrows that home by residing in it.
- **The value-level description stays.** Whole-value carriers (delivery
  envelopes, binding tables) keep their single stored description — the
  get-or-mint of the union over the value's runs, cheap under interning.

## String residence

Every string a value-family slot holds — a `KObject::KString`, a `Tagged`
discriminant, a `KKey::String` dict key — is a `&'a str` bumped into the
region the value lives in
([`RegionBrand::alloc_text`](../src/machine/core/arena.rs), over workgraph's
[`RegionHandle::bump_text`](../workgraph/src/witnessed/region.rs)). The slot
owns no allocation, so it runs no destructor at region death and the bytes go
with the bump's chunks; that is what makes the slot `Copy` and `deep_clone` a
pointer copy for the string arm. There is no interning table: a run-global one
is run-rooted state koan does not keep, and a per-region one is itself
`Drop`-bearing state inside a region being driven `Drop`-free. Comparison and
hashing stay `str` compares, so a key produced in one region matches a key
produced in another by content.

The bump keeps **no address table**, so nothing can ask which region a `&str`
points into. One rule follows, and every string path is an instance of it:

> A path that claims a region's **release** re-bumps the bytes at its
> destination. A path that **pins** shares the pointer verbatim, under the
> witness the enclosing fold already composed.

Re-bumping is what makes the empty-reach verdict above honest: a string cell
that named no region while still pointing into a retiring one would dangle with
no fold able to rescue it, and no residence audit could catch it. So every
substrate door re-homes a top-node string cell and every string dict key before
the verdict is read, the tagged door re-homes its discriminant into the same
region as its payload substrate, and the copy verb re-bumps at the destination —
which is what keeps the relocation's release-exact answer exact. Pinning paths
(a retaining adoption, a projection's `deep_clone`) share the pointer, covered
by the reach that already names the producer region.

Two gates keep a bare string off the paths that cannot honour the rule. A
string is **not** a shallow scalar, so a string producer takes a fold door
rather than the no-fold arm that rebuilds owned at `'static` — there is no
`'static` rebuild to make. And the residence audit answers `false` for a
string, so no bare string crosses a runtime-audited move-in; a copying adoption
routes to the same rebuild-through-a-destination-door path a substrate carrier
takes.

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
  contribute their own memoized cost, borrow leaves contribute zero. An
  expression cell is one of those borrow leaves: it holds its node by value, and
  the node's parts run, keyword text and structural cache all live in the
  eternal-tier storage that parsed them, so copying the cell copies pointers and
  rebuilds nothing. **Every cell family prices**, so the decision below is never
  taken blind. Because substrates are immutable
  the memo can never go stale, and because the copy verb rebuilds a shared
  subvalue once per reference, the memoized sum is the copy's *exact*
  cost — no forwarding map, no walk.
- **the region's allocated total** — its arenas already know their size, and
  its bump reports its live occupancy
  ([`Region::bump_bytes`](../workgraph/src/witnessed/region.rs)), so the string
  bytes a pin would retain are in the denominator too.

For a priceable value crossing out of its **own home region**, the rule is that
ratio: copy when `copy_cost < α × region_allocated` — "this value is a small
fraction of what the pin would retain." A value that is most of its region pins
(retention barely exceeds the value; the copy would be pure CPU); a small result
escaping a fat frame copies and releases it. α is a tuning constant of the seam,
not observable in language semantics. A **foreign crossing** — the value is
resident in a region the producer host does not own — always pins: pricing a
copy-out at an intermediate host is region evacuation's job, not the
per-crossing seam's.

The ratio is gated by the exact **borrows-home** query. Set, the value **pins
outright** — a leaf provably borrows the home region, so a copy would pay the
rebuild *and* keep the pin; the ratio is never consulted. Clear, the copy
provably releases the host (no surviving borrow reaches it), so
the ratio alone decides. This is why borrows-home is a *separate*, sharper
question than contains-borrows: contains-borrows asks only whether
*any* borrow leaf exists into *any* region, and remains the seal/reach
conservatism input; the copy decision needs the home-relative question, and gets
an exact answer. Release is a stored fact on either verdict: the copy claims
the retiring host's release exactly when no surviving run description names it
([§ Sectioned reach](#sectioned-reach)), so a value whose leaves all point into
foreign regions still releases its home.

A **pinned record** shares its producer-resident substrate by a pointer-copy
(never a partial rebuild), made at the destination's own fold brand. Because a
record's substrate borrow carries no borrow naming its *own* home region, the pin
verb hands the fold a retention claim that keeps **every** region the source
envelope named — rather than leaning on ambient coverage the way a closure's
captured region does — so the composition names the producer region as a member of
the product's reach and retains it here. The explicit naming is
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
([reach.md § The pin bundle](../workgraph/design/reach.md#the-pin-bundle)).

Expression parts are not in it either, for the same reason. Both node families —
the raw AST [`KExpression`](../src/machine/model/ast.rs) and the scheduler's
[`WorkingExpression`](../src/machine/model/ast/working.rs) — are `Copy` handles
over bumped slices of `Copy` parts, so no expression slot carries `Drop` glue and
region death for a spliced node's part storage is chunk deallocation. Their
storage tiers differ and that difference is what the value channel reads:

- **Raw AST lives in program storage**, a `FrameStorage` at the eternal tier
  minted by [`program_storage`](../src/machine/core/arena/frame.rs) above the run
  root ([memory-model.md](memory-model.md)). The eternal rule filters such a
  member out of every pin bundle and reach description, so a value pointing at
  program text reaches nothing.
- **A working node's parts are bump-allocated in the dispatching step's region**,
  because that is where the scheduler writes a resolved sub-result's resting
  `Sealed` cell when it splices one in.

`KObject::KExpression` takes a `KExpression`, and there is no conversion the other
way, so a resolved sub-result is **unreachable from the value channel** by typing
rather than by audit. That is what lets the alloc door call an expression cell's
run empty, `retains_home` answer `false` for one, and the cost memo price it at
zero as a borrow leaf ([§ Cost-driven copy](#cost-driven-copy-the-optimization)) —
each an O(1) read of a structural fact, with no walk over the node behind it.

## Invariants preserved

- **Cycle-freedom needs no gate.** No stored value owns an `Rc` back to any
  region — a substrate borrow is a borrow, a reach's pins are holder-owned
  and the self and eternal rules keep ownership acyclic
  ([reach.md § Composition](../workgraph/design/reach.md#composition-minting-a-description-and-retaining-its-pins))
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

- [Region evacuation at frame death](../roadmap/untyped_arena/region-evacuation.md)
- [Drop-free region death](../roadmap/untyped_arena/drop-free-region-death.md)
