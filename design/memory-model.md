# Memory model and scoping rules

Every `KObject` lives in a [`KoanRegion`](../src/machine/core/arena.rs). Top-level
work allocates into the **run-root region**; each user-fn call gets its own
**per-call `KoanRegion`** owned by [`CallFrame`](../src/machine/core/arena.rs),
freed when the call's slot finalizes. Above both sits
[`program_storage`](../src/machine/core/arena/frame.rs), the region program text and
its parsed AST are bumped into: created before the run root and dropped after it, at
the same **eternal** tier (`PinsRegion::needs_no_pin`), so a value pointing at
program text names no member any pin bundle has to hold. It stays outside the frame
lifecycle entirely — no `CallFrame` adopts it, no `Scope` lives in its region,
and its only capability in use is the bump its `ProgramBrand` hands parse output and the one runtime
body that synthesizes a value-channel node
([§ Value-channel AST](value-substrates.md#value-channel-ast-the-program-storage-marker)). The target storage model for composite
value substrates — region-resident payloads, witnessed-only construction, the
pin-versus-copy escape policy — is pinned in
[value-substrates.md](value-substrates.md).

## Storage shape: a graph of region slots

A `KoanRegion`'s value storage is one **bump** and nothing else. Every value
family is `Drop`-free by construction and lives there, where death is chunk
deallocation and no per-slot glue runs at all: the `KObject` and `Held` cells, the
four container substrates (record, list, dict, payload) with their index metadata,
the `KFunction` and `Module` families, `Scope` itself, and the strings and
expression parts already hosted there
([value-substrates.md § Untyped arenas](value-substrates.md#untyped-arenas-the-drop-free-end-state)).
`Scope`'s `Drop`-freedom is structural rather than audited: every field is `Copy`, a
`Cell` of a `Copy`, or a bump-backed table whose own vacuous destructor is
suppressed, and the `reattachable!` declaration in
[arena.rs](../src/machine/core/arena.rs) states that as a compile-time
`!needs_drop::<Scope<'static>>()` assert — a field that later brings glue back fails
the build there.
A `KType` and a `TypeIdentifier` take neither tier: both are `Copy` handles — an
interned registry index and a borrow of name bytes already resident where they
were parsed — so the type channel's carriers hold them by value. Slots have stable
heap addresses; the runtime carries cross-references between them rather
than ownership trees. The structural edges:

- `Scope.outer: Option<&'a Scope<'a>>` — the lexical-parent chain, held
  outright. Many sibling scopes can share one outer, so the
  in-degree is unbounded.
- `Scope.region: &'a KoanRegion` — back-pointer to the owning region.
- [`Bindings.data`](../src/machine/core/bindings.rs) maps each bound name
  to a `&'a KObject<'a>`. The pointee may live in this scope's region or in
  an outer one.
- [`KFunction.captured`](../src/machine/core/kfunction.rs) holds the closure's
  definition scope as a plain `&'a Scope<'a>` — a bumped `KFunction` is never
  erased, so the field is already at the region's `'a`. Multiple
  `KFunction`s share one captured scope when they were defined in the same body.
- `KObject::KFunction(&'a KFunction<'a>)`
  holds a bare value-side reference to a function-region slot and reaches the
  per-call region that owns the function's captured scope only through that
  scope's region owner, read off the region's own host back-link. It carries no
  per-value liveness anchor:
  the region an escaping closure reaches is pinned by its envelope's
  witness [`FrameSet`](../src/machine/core/arena.rs) bundle while it rides a
  scheduler slot, then minted — description into the consumer region's reach
  table, pins onto the binding entry — when the value is bound (see
  [§ Region lifetime erasure](#region-lifetime-erasure)).
- `Module` and `Signature` cache their declaration scopes as a plain
  `&'a Scope<'a>` (heap-pinned by the surrounding region chain). Both are bumped
  and so never erased: the field is already at the region's `'a`. A `Module`
  additionally holds its bumped `path` and its two frozen bump-backed member
  tables, all
  hosted in that same region.

**Directionality rule.** References go inward freely — a per-call region's
slots may point at run-root slots, because the run-root region outlives every
per-call region by the lexical-scoping invariant. A reference that points
*outward* — a value referencing a slot in a dying per-call region, the
canonical case being a closure / module returned from its defining frame —
keeps that region alive through its holder's pins, never a per-value anchor
on the value itself: a producer slot's `FrameSet` pins it while the value rides
the scheduler, and a bound value's pins ride its binding entry, minted with the
reach description when the value is bound (see
[§ Region lifetime erasure](#region-lifetime-erasure)).

**Why graph rather than tree.** Many-to-one captures and bindings, sibling
scopes sharing an outer, mutual references between a `Scope` and the
region's bump it lives in, and cross-region carrier-witness pins all
break tree shape. Slots are added incrementally as the program runs;
references can be installed before or after the pointee exists (forward
declarations, dispatch-park edges). The frame-chain `Rc` that rides on top of
this graph lives in
[per-call-region/README.md](per-call-region/README.md).

The graph shape is also why the runtime stores `*const T<'static>` and
transmutes on access: a self-referential graph of incrementally added
slots with cross-references doesn't fit the one-owner-builds-one-dependent
shape that self-referential-struct crates model.

## Scoping: lexical

Free names in a user-fn body resolve through the function's **definition**
scope, carried on [`KFunction.captured`](../src/machine/core/kfunction.rs) — not the
call-site scope. Top-level `FN` definitions capture the run-root, so their free
names resolve through it; nested `FN`s correctly close over their enclosing
locals.

Lexical scoping is what makes the F_{k+1}→F_k chain in tail-recursive code O(1)
memory. Without it, a recursive call would resolve the recursive name through
the call-site scope and pin every prior frame's bindings alive.

## Per-call region protocol

The per-call region's lifecycle — how a copied value's reached regions are
kept alive (the [`copy_carried`](../src/machine/execute/lift.rs) fold copy plus the
carrier-set reach read off each dep's witness for both channels), how the scheduler propagates the active frame, how
builtin-built frames chain the call-site frame's storage through
`FrameStorage.outer`, and how the TCO step reuses the frame shell over a
fresh `FrameStorage` — is documented in
[per-call-region/README.md](per-call-region/README.md). This file
keeps the storage-shape, scoping, and lifetime-erasure scaffolding the
protocol sits on top of.

## Region lifetime erasure

No **value** stored in a [`KoanRegion`](../src/machine/core/arena.rs) is erased at
all. The region's storage is its bump, and a bump's own type carries no lifetime, so
`'a` enters only at the allocating call: a value whose fields are already at the
caller's `'a` — its region back-pointer included — is built there and placed there,
with no `'static` round trip to audit and so no residence obligation to discharge
([§ Move-in residence](#move-in-residence)). `KoanRegion` still carries no lifetime
parameter of its own, and the generic engine still lives in the
[`Region<W>`](../workgraph/src/witnessed/region.rs) substrate (`KoanRegion` is the
Koan instantiation `Region<KoanStorageProfile>`), which declares only the
frame-owner type its reach descriptions are typed at.

What *is* erased is a **reference**: a scope pointer a carrier stores lifetime-free
across a scheduler slot, and the freshly bumped reference the crossing construction
door re-anchors on its way out of a `for<'b>` brand. Both route the scheduler's
single audited `erase_to_static` / `retype` pair (described below) over a
`Reattachable` family (`At<'static> == Self`), the GAT both directions key on, so
the region side and the scheduler side share one transmute rather than each carrying
its own. It is sound because:

- Lifetimes are zero-sized, so `T<'a>` and `T<'static>` have identical layout.
- The re-anchor returns an `&'a` bounded by a live borrow of the region the pointee
  was bumped into; no `'static` reference ever escapes.
- A bump never moves an allocated chunk, so a held `&KoanRegion` keeps every pointee
  it has handed out at a fixed address for the whole of that borrow.

The bump ([`Region::bump`](../workgraph/src/witnessed/region.rs)) is the storage home
for every `Drop`-free value that names the region's own lifetime, which is to say
every value family Koan has. The library bumps its own container metadata there (the
reach-run partitions and cell index blocks a sectioned container names) and Koan
bumps its value cells, its container substrates, its substrates' index metadata,
its strings and its expression parts beside them. A value with operands whose reach
the product must carry reaches the bump through
[`FoldedPlacement::fold_and_bump`](../workgraph/src/witnessed/bump.rs) or the
[`allocator`](../workgraph/src/witnessed.rs) an enclosing fold's placement already
hands out, either of which composes the stored value's reach in the same call; a
bytes-only or keyed-index allocation reaches it through the same
[`BumpAllocator`](../workgraph/src/witnessed/bump.rs) verbs off the handle
(`allocator().text` / `.value` / `.slice`), which is where those verbs are defined
once for every surface.

An entry may hold an `&'a` back into the same region with no residence audit at all.
The `T: Copy` bound on the write-once verbs is what keeps that honest — a bump
releases its chunks whole and runs no destructor, so admitting a `Drop`-bearing entry
would silently skip one. The shapes that are glue-free without being `Copy` take
verbs of their own, each carrying a monomorphized
`const { assert!(!needs_drop::<_>()) }` in place of the bound: a frozen keyed index
takes [`BumpAllocator::frozen_table`](../workgraph/src/witnessed/bump.rs), which
allocates the buckets over the very allocator it places the header through, and a
value that keeps *mutating in place* through interior mutability — a `Scope` — takes
[`BumpAllocator::in_place`](../workgraph/src/witnessed/bump.rs). A **collection** is
where the bound stops travelling with the bytes: a table that keeps mutating — a
scope's binding tables, its SIG slot collector — is built over the allocator's raw
`allocator-api2` seam, wrapped in `ManuallyDrop` so its vacuous teardown is
suppressed, and its writer owes the entry-glue assert at the declaration naming the
entry types. Either way the destructor forgone would have freed only bytes region
death frees anyway.
Cross-references among bumped entries need no drop-order argument at all:
everything there dies with the region, at once.

The scope-pointer case — `CallFrame`, `Module`, `Signature`, `KFunction`, and a `Scope`'s
own lexical parent each holding a pointer to a captured, defining, or parent `Scope` — holds that
scope **outright** as a plain `&'a Scope<'a>` (a thin pointer, layout-invariant in `'a`), centralized
through the [`ScopeRefFamily`](../src/machine/core/ref_carriers.rs) reattach family in
[`ref_carriers.rs`](../src/machine/core/ref_carriers.rs), with no scope-specialized re-anchor helper — the
embedded pointer re-anchors with the holder's own whole-value retype.

No holder is erased on the way in, so every embedded scope reference is already at the region's
`'a` with no retype involved at all — a bumped `Scope`, `KFunction`, `Module` or `Signature` alike.
Where a re-anchor does happen it is over an **erased reference**, not a value: a scope pointer read
back out of a lifetime-free carrier, re-anchored in one `Reattachable` retype against the witness
that pins it. Either way
`KFunction::captured_scope`, `Module::child_scope`, `Signature::decl_scope`, and a
`Scope`'s `outer` / `root` are **bare field reads** of an already-`'a` reference, not scope-specialized
re-hands. The scope / module / function path carries **no `unsafe`** of its own — the only retype it
routes is the substrate's single [`retype`](../workgraph/src/witnessed.rs), shared with every other carrier;
there is no per-handle `NonNull` deref.

At construction the scope reference is coupled at its target lifetime with no scope-specialized
re-anchor verb. A same-region child stores its already-`'a` parent by plain coercion — the
constructors take `&'a Scope<'a>`. A per-call child, whose lexical parent / root is longer-lived,
builds through the externally-witnessed construction door
[`build_frame_child_witnessed`](../src/machine/core/arena.rs): it brands the fresh region and the
foreign parent at one `for<'b>` (the `zip`-combined [`SealedExtern::open`](../workgraph/src/witnessed.rs) the
run-loop step also rests on), builds the real invariant `Scope<'b>` coupling them through
[`Scope::child_for_frame_witnessed`](../src/machine/core/scope.rs), and erases it witness-less — so
`CallFrame::new` builds the per-call child at real (non-`'static`) lifetimes with
no construction-time fabrication and no re-anchor outside the witnessed substrate.

`CallFrame`'s per-call child scope (non-generic — it backs `Rc<CallFrame>`) and a scheduler slot's
`NodeScope::YokedChild` (a cart-ancestor block scope evicted off the lifetime-free node) additionally
ride the substrate's externally-witnessed [`SealedExtern<ScopeRefFamily>`](../workgraph/src/witnessed.rs)
carrier — a `&'static Scope` erased once on the store side through the safe
`erase_to_static::<ScopeRefFamily>` (forgetting a reference's lifetime for storage cannot fabricate
one). Both are read through the carrier's **rank-2** [`SealedExtern::open`](../workgraph/src/witnessed.rs) (the
frame's `with_scope`): the scope opens at a `for<'b>` brand against the frame / cart `Rc`, so the
fabricated lifetime cannot escape the window and no scope borrow rides up a `&mut self` path.
[`SealedExtern::open`](../workgraph/src/witnessed.rs) (plus its consuming externally-witnessed twin) is the
**single access verb**: every frame-side and seed-side read folds onto it, and the borrow-bounded
`attach` re-anchor — a `<'w, 'b: 'w, W: Witness>(&'w self, &'w W) -> &'w Scope<'b>` that handed back a
free content `'b` the brand cannot — is deleted.

Beyond the store-side erasure and the branded scope pointers, a handful of carriers store a
borrow-carrying *value* on a structure the borrow checker cannot lifetime-track — a scheduler
node's slot, a per-call `TraceFrame` — and re-anchor it at a caller-chosen lifetime on read,
witnessed by a held `Rc`. The erase/reattach discipline that makes the move safe lives in the
top-level [`witnessed`](../workgraph/src/witnessed.rs) module, a sibling of `machine` and `scheduler` that
names no concrete workload type: both depend on it for the machinery, not the reverse.
[`witnessed.rs`](../workgraph/src/witnessed.rs) declares `unsafe trait Reattachable { type At<'r>; }` —
a family whose representation is identical across every choice of its single lifetime — and
[`Erased<T>`](../workgraph/src/witnessed.rs) stores that family's `At<'static>` form. A single
private `retype<A, B>` — a `transmute_copy` through a `ManuallyDrop` (plain `transmute` cannot prove
two opaque GAT projections share a size), guarded by a `const` size assert that restores the check
`transmute` would emit — is the only place a
`T::At<'a> → T::At<'b>` lifetime retype is written; `Erased::erase` / `Erased::reattach`, the
externally-witnessed `SealedExtern::open`, the `Witnessed` accessors, and the region's
store-side `erase_to_static` all route it. The carrier families live beside their own
types as declarative `unsafe impl Reattachable` instantiations — `CarriedFamily` /
`ContinuationFamily` for the scheduler value (`Workload::Value`) and continuation
(`Workload::Continuation`), and `ScopeRefFamily` so the frame / node `&Scope` carriers and the
region's `&Scope → &Scope<'static>` storage erasures route the same primitive — so `witnessed.rs`
names no concrete Koan type and the scheduler stays workload-independent (the workload depends on
the substrate for the machinery, not the reverse).

[`Witnessed<T, W>`](../workgraph/src/witnessed.rs) bundles an erased carrier `Erased<T>` with the liveness
witness `W` that pins its pointee in one value, so "the witness keeps the value alive" is a type
invariant rather than a co-stored field pair plus a SAFETY comment. `W` is a [`Witness`](../workgraph/src/witnessed.rs)
— an `unsafe` marker asserting its pointee stays at a fixed address while held; `Rc<F>` qualifies
(a static `StableDeref` assert records the obligation), and a *set* of them — the Koan result-slot
and lift witness [`FrameSet`](../src/machine/core/arena.rs) — pins every region a value reaches at
once, an empty set being a frameless / run-region terminal whose backing outlives the carrier. The carrier is re-anchored through one
of three read/transform accessors, all sound by construction: `with` re-anchors behind a **rank-2**
`for<'b>` brand so the fabricated content lifetime cannot escape the closure into the result (the
generativity trick; the naive content-free reattach is a Miri-proven use-after-free); `map` consumes
and re-projects under the same brand and witness (`yoke::map_project`'s shape); and `read` hands the
carrier out bounded by the `&self` borrow itself, sound because the content lifetime *is* the borrow
the bundled witness pins, not a free `'b` the caller could widen. Two build-time accessors close the
co-location gap `new` leaves to caller assertion: `yoke` *sources* a carrier from the witness's own
region behind a `for<'b>` brand (over the `WitnessRegion` trait), so the only references the carrier
can hold are region-derived — the witness-pins-the-value invariant holds by construction rather than
asserted; and the envelope merge (`Delivered::merge_into` / `transfer_into`) combines two carriers
under one shared brand, runs a binding projection, and re-seals under the *composed* witness — the
union of both operands' regions, with `outer`-chain subsumption dropping a region another already
pins. The pin covering the shared re-anchor comes from the envelopes themselves — each side travels
with its own pins, so no caller threads a pin in — and the crate-private engine that consumes it
(`merge_composed`, or `merge_staged_composed` where the source side is a whole staged run — see
[witnessed-memory.md § Storage and access](../workgraph/design/witnessed-memory.md#storage-and-access-seal-open-transfer_into))
is reachable only from those verbs. The composition is `ComposeWitness::compose`,
run inside the brand with the destination in scope: an owned region set composes by plain union
(total, since a set can always represent the combined pin), while a hosted carrier mints the combined
reach into the destination's own arena. All keep their `unsafe` retype inside the module, so callers
carry none; `yoke` in fact routes only the safe `erase`, carrying no retype of its own.

The value channel is borrow-checked end to end. `finalize` takes the workload's finished terminal
as a [`Delivered`](../workgraph/src/witnessed/delivered.rs) envelope bundling the erased value under
a [`CarrierWitness`](../src/machine/core/carrier_witness.rs) — the **reference-only** carrier, a
reference to the value's reach description and nothing else, pinning nothing itself; the description
is where both of the value's region facts live, its host region and the regions its borrows reach —
sealed **as-is** (a declared return is checked and re-stamped in place first): there is no
Done-boundary relocation or sever gate. The envelope is internal transit: the delivery walk adopts
the terminal into each edge's destination region, once per distinct destination
([dag-scheduler.md § Delivery at finalize](../workgraph/design/dag-scheduler.md#delivery-at-finalize)),
with the retention predicate deciding per destination whether the value deep-copies out — freeing
the producer frame at finalize — or stays resident in the producer's region, that frame transferred
into the destination region's union bundle. A filled edge keeps its resident in `Retained` — the
opaque dormant tier with **no read verb at all**, which hides every transform and re-anchors only
through a rank-2 destination verb — re-entering circulation only through `Delivered::lift` under
the destination's own liveness, inside a `for<'b>` brand whose fabricated content lifetime is
un-nameable, so nothing branded escapes into a result and no re-anchored reference rides a `&self`
borrow up-stack. The continuation — droppable, so it rests on
the substrate's **owned tier** as a `SealedPinned` sealed against the slot's anchor `Rc` at the
scheduler's install door — re-anchors through the drain step's single consuming open:
[`Host::step`](../src/machine/execute/harness.rs) opens it beside the active-scope operand at one
rank-2 `for<'b>` brand standing in for the step lifetime, witnessed by the seal's own bundled anchor
pin plus the step's owned pins, so the whole tail nests inside the brand and carries no loose
witness-borrow reattach. That brand is **bounded above by `'run`**: the open hands its closure a
[`Within<'b, 'run>`](../workgraph/src/witnessed/dormant.rs) token whose declared `'run: 'b` the
`for<'b>` instantiation discharges, which is what lets the run's
[`ProgramBrand<'run>`](../src/machine/core/arena/frame.rs) — a live borrow, not a sealed carrier —
be stored unshortened where the step's
[`DecideCtx`](../src/machine/execute/decide/ctx.rs) is built: the context keeps `'program`
distinct from the step lifetime, related only by its own `'program: 'step` bound, and the token is
what discharges that bound. The brand is invariant, so no shortening is available to it; what
reaches step code instead is the covariance of what it *mints*. Program storage therefore reaches a
builtin body with no carrier, no pin and no `unsafe`
([witnessed-memory.md](../workgraph/design/witnessed-memory.md)). The
dep channel re-anchors at a *node* lifetime, not a fabricated `'run`: a dep is delivered before its
consumer's step starts, arriving as an ordinary resident of the consumer's region — no envelope
rides the step, and no in-band dep carrier exists. The landing crosses the delivery adopt's own
fold, whose brand is where [`copy_carried`](../src/machine/execute/lift.rs) copies a
copy-verdict value into the destination region with a plain `'b → 'b` structural alloc — the
composite spine sharing its `Rc` payloads, a closure / future / module riding its bare `&'b` borrow
into the source region — so every dep value is born at a brand its own carrier supplied, never
through a free reattach. There is **no value-path `unsafe`**: the relocation allocs at the
destination region's own lifetime, so the lift hook is a safe `deep_clone` through the
destination's own fold door, and the adopt is a mint — the relocated value re-sealed with its reach
(and, for a value borrowing into the dying producer, that producer) minted against the destination,
description into its reach table, pins to the destination's union bundle. The run's roots are
Koan-held edges destined into the run-global root region, so top-level terminals are delivered
there at finalize and the drain boundary in
[`run_program`](../src/machine/execute/interpret.rs) reads each as a resident of a region
the run owns, releasing the root edges when it is done.

A relocated closure / future / module survives its producer's dying frame because the copy keeps its
bare borrow and the *consumer* keeps that borrow's region alive. Both channels carry the regions they
reach on their [delivered carrier](../workgraph/design/witnessed-memory.md#storage-and-access-seal-open-transfer_into): a
**closure / future** seals its captured-scope reach at construction, and a **module value** names its
child scope's own region (composed by the module store fold
[`Scope::store_module_object`](../src/machine/core/scope/reach.rs)), which owns the union covering
everything its members reach. The embedding or binding site mints that
carrier's reach into its own arena (`transfer_into` at an `attr` / `FROM` projection,
[`Scope::adopt_for_binding`](../src/machine/core/scope/reach.rs) at a `let` / user-fn arg / `USING`
bind), and delivery mints a root terminal's full reach against the run frame's own region — the
destination its root edge names, whose union bundle owns the pins — so a
multi-region value keeps *every* region it reaches, read straight off its carrier rather than
reconstructed from the value. A minted description is **exact**: it names the value's whole reach,
never a reach narrowed against what some destination already pins. Acyclicity comes from the self
and eternal rules instead
([reach.md § Composition](../workgraph/design/reach.md#composition-minting-a-description-and-retaining-its-pins)).

The per-call frame's seed binds (MATCH / TRY `it`, `KFunction::invoke` params, the deferred-return-type
elaboration) open the child scope at a `for<'b>` brand through
[`CallFrame::with_scope`](../src/machine/core/arena.rs) and **relocate** their caller value into the
opened scope's own region through the substrate before binding it — the `it`-bind and param-bind via
[`Scope::adopt_for_binding`](../src/machine/core/scope/reach.rs) (which relocates the value into the
frame region at a fold brand, the fold's composition minting and retaining what the copy still
reaches, rather than assuming purity — see
[§ Move-in residence](#move-in-residence)), the deferred return re-homing
its elaborated `KType` into the captured-scope region — so the
seed fabricates no free `&'a`. The store
side carries no `unsafe` at all: forgetting a scope reference's lifetime for storage routes the safe
`erase_to_static`, and a region-resident holder's embedded `&Scope` re-anchors with the whole value on
read, both deferring every fabrication hazard to the witnessed retype.

The allocation engine needs **no cycle gate**: a stored value holds no owning `Rc` back to a region —
a closure / future / module is a bare borrow into its defining region, kept alive by its holder's
pin bundle rather than an embedded anchor — so storing it where requested can never close an
allocation back-edge. Nor is there a store engine to gate: every family lands in the region's bump
through the same [`BumpAllocator`](../workgraph/src/witnessed/bump.rs) verbs, and the allocation
*capability* is what stays unbypassable — the region's own `allocator` is `pub(crate)` to workgraph,
so a bare `&KoanRegion` has no allocation surface and only a `RegionHandle` minted from a region
owner (or handed out at a `for<'b>` brand) can write.

A [`CallFrame`](../src/machine/core/arena.rs) is a thin shell over a refcounted
[`FrameStorage`](../src/machine/core/arena.rs): the shell carries a `Rc<FrameStorage>` and an
`Option<SealedExtern<ScopeRefFamily>>` (the child scope; `None` only transiently during construction), while
`FrameStorage` bundles the `KoanRegion` and an `Option<Rc<FrameStorage>>` for the parent-frame
chain. The shell/storage split lets an escaping value pin only the storage (its region), so the
region outlives the shell independently — a `FreshTail` tail hop drops the shell outright
while the escapee keeps its region snapshot alive (see
[tail-call-optimization.md](tail-call-optimization.md)). Two
invariants make the ownership unit coherent:

- **Heap-pinning via `Rc`.** `CallFrame::new` builds the region inside its own
  `Rc<FrameStorage>` and only ever exposes the frame as `Rc<CallFrame>`, so the inner
  region's heap address is stable for the storage Rc's life and `scope_carrier` (a
  `&'static Scope` into `region.scopes`) stays valid alongside it. Accessors re-attach lifetimes
  anchored to `&self`. A tail reset installs a *fresh* `FrameStorage`, so the region
  address changes across a reset — no accessor captures it across one, and the borrow
  checker forbids safe code from doing so.
- **Field declaration order encodes drop order.** On `FrameStorage`, `region` is declared
  before `outer` so the auto-derived `Drop` tears down this frame's region *before*
  releasing the parent storage Rc; on the shell, `storage` is declared before `scope_carrier`.
  Inner references die before the outer storage they may reference, ruling out a dangling
  `outer` during drop.

### Move-in residence

Every move-in of a value into a region is discharged **by the door's signature**. There is no
structural residence walk over a composite value, and no address side table anywhere for one to
consult: a door either takes a shape that cannot name a foreign region, or takes its input at a fold
brand no ambient borrow can inhabit.

The **region-free leaf doors** are the first kind. [`Scalar`](../src/machine/model/values/kobject.rs)
— `Number` / `Bool` / `Null` — is region-purity expressed as a *type*, so
`RegionBrand::alloc_scalar` admits nothing that borrows a region and rebuilds each arm from its owned
payload; `RegionBrand::alloc_string` is its sibling for the one leaf whose *representation* is
region-hosted, re-homing the bytes into this region as part of the store; and
`RegionBrand::alloc_expression` takes a
[`ProgramExpression`](../src/machine/model/ast/program.rs) and nothing else — the marker minted only
by a [`ProgramBrand`](../src/machine/core/arena/frame.rs) door — which is what proves the cell it
bumps borrows only eternal-tier program storage. Each yields a resident `&'a KObject<'a>` bumped in the destination, so residence is where
the door placed it. The witnessed spellings (`alloc_scalar_witnessed`,
`alloc_expression_witnessed`) seal that product under a member-less own-region description: the empty
witness pins nothing, so its carrier is sound only as a within-step transient — the step doors return
it wrapped as a [`StepCarried`](../src/machine/execute/step_carried.rs) branded at the step's `'step`
lifetime, so the borrow checker rejects any attempt to stash it past its construction step and the
sole exit to node storage is finalize's fold.

A carrier-less argument — one the `arg_carriers` contract calls region-pure — is placed through
[`Scope::place_pure_value`](../src/machine/core/scope/reach.rs), which routes exactly those shapes to
exactly those doors. Every other shape borrows a region the door cannot name, reaches its destination
as a delivery envelope instead, and arriving here is a construction bug the door reports as a
diagnostic rather than a residence verdict.

`KType` reaches no door at all. A `KType` is a `Copy` content-digest handle
([`ktype.rs`](../src/machine/model/types/ktype.rs)) — a bare `u128` naming a node the run-frame
registry owns ([type-registry.md](typing/type-registry.md)) — so it holds no region pointer, needs
no store door, and has no residence to audit. A type crosses a region boundary as a plain handle
copy, and content-digest identity is preserved because the handle *is* the identity. The binding
tables therefore store `KType` by value ([`bindings.rs`](../src/machine/core/bindings.rs)), with no
reach evidence and no borrow to witness.

A `Drop`-free region-borrowing `Module` takes the plain bump verb
`RegionBrand::allocator().value` alongside those leaves, because it is `Copy` and nothing about it is
erased on the way in; the door derives its brand from the value's own anchoring scope and re-homes
the value's bytes at that same brand (see below).

Everything else takes one of the two **rank-2 brand** doors — the second kind, where the input is
taken at a `for<'b>` lifetime no ambient borrow inhabits. A `KObject` embedding a substrate, a
`KFunction` and its wrapper, a re-tagged `Module` and a relocated container ride the *folded* one; a
`Scope` itself is *born* at its destination:

- **folded** (`FoldingBrand::alloc_object_folded` / `alloc_cell_folded` / `alloc_substrate_folded` /
  `alloc_module_folded` / `alloc_function_folded`) — no runtime audit at all,
  sound by signature: the sink takes its input at the brand lifetime (`KObject<'b>` on
  `FoldingBrand<'b>`), and inside a fold combinator's `for<'b>` closure the only inhabitants of that
  lifetime are values derived from the fold's declared operand views, the brand's own allocations, and
  owned/`'static` data — all named by the witness the enclosing combinator composes. An
  ambient-lifetime capture cannot coerce to `'b` (which has no outlives relation to any enclosing
  lifetime), so smuggling a captured borrow past a folded sink is a compile error rather than a
  runtime-audited obligation. `FoldingBrand`'s sole constructor
  ([`in_fold_closure`](../src/machine/core/arena.rs)) takes a
  [`FoldedPlacement`](../workgraph/src/witnessed.rs) — a compile-only capability privately wrapping
  the destination handle — which only a fold engine (`transfer_into` / `transfer_all_into` /
  `merge_into` / `Delivered::project` / `StepAllocator::alloc_carried_with`) mints over the
  destination region
  and whose `'b` brand keeps it from escaping the closure, so the capability is reachable only at a
  fresh fold brand. The store itself is the placement's own
  [`bump`](../workgraph/src/witnessed.rs) door: a `KObject`, a `Held` cell, a `Module` and a
  `ContainerSubstrate` are all `Copy`, so the cell lands in the destination's bump and the brand's
  `'a` — the fold's own — is what discharges the residence obligation at compile time.
- **born** ([`build_frame_child_witnessed`](../src/machine/core/arena.rs),
  [`Scope::alloc_child_transparent`](../src/machine/core/scope.rs)) — the same rank-2 argument for
  the two stores that embed an operand living in *another* region, which no destination brand can
  derive: the per-call frame child's foreign lexical parent, and the transparent `USING` window's
  binding table in the opened module's region. The library door
  ([`RegionHandle::bump_born_with`](../workgraph/src/witnessed/region.rs)) hands the construction
  closure a `FoldedPlacement` over the destination at a fresh `for<'b>` with the operand
  re-anchored to that same `'b`; the closure builds and the door bumps in one act, returning the
  resident at the handle's own `'a`. The operand crosses as a `SealedExtern` under a witness pin
  borrowed for the destination region's `'a`, which is what keeps the door a lifetime *shortening*.
  Every koan door derives its destination handle from the value's own anchoring scope, so a
  mis-paired store is unstateable.
- **direct** ([`Scope::alloc_child_under`](../src/machine/core/scope.rs) and its same-region
  siblings, [`Scope::alloc_run_root`](../src/machine/core/scope.rs)) — no brand and no re-anchor at
  all. Every field the value takes is already at the destination's own `'a`, so it is built there
  and placed through [`BumpAllocator::in_place`](../workgraph/src/witnessed/bump.rs); residence is
  the borrow checker's, since the only region reachable through the anchoring scope's brand is the
  one the value lands in.

Every move-in that lands a value reaching *another* region takes the folded tier — the binding and
adoption doors ([`Scope::adopt_for_binding`](../src/machine/core/scope/reach.rs),
`Scope::adopt_carried`), the module store folds (`Scope::store_module_object`,
`Scope::store_transparent_view`). Each is a fold or a merge whose composition mints the product's
exact reach into this scope's arena and retains the owning bundle there for the region's life, so the
witnessed product is already the finished carrier: the reach is never a free parameter a caller
pairs with a value. That is structural — there is no constructor that pairs a loose reach with a
value. `PinBundle` is crate-private to `workgraph` and owned pins cross the boundary only as an
opaque `StepCoverage`, so Koan cannot assemble, widen or narrow a claim at all
([reach.md § The carrier states](../workgraph/design/reach.md#the-carrier-states)); what it holds is
whatever a composition derived for a specific value.

A **registered callable** and an **operator-group record** are placed by such a composition too, so
the registration doors state no reach of their own.
[`KFunction::alloc_captured`](../src/machine/core/kfunction.rs) is a witnessed birth: the captured
scope, the signature already minted at that scope's own brand, and the body cross as one resident
seed operand, and the callable is assembled *inside* the `merge_into` fold that stores it. The fold's
`for<'b>` brand is the residence proof — an ambient region borrow cannot inhabit `KFunction<'b>` —
and the merge composes the product's description: hosted in the captured scope's region, that region
its one member. A group record's birth is a *yoke* instead
([`Scope::birth_operator_group`](../src/machine/core/scope/reach.rs) around
[`OperatorGroup::alloc`](../src/machine/model/operators.rs), which re-homes every byte at the brand
it is handed), so its composed description names the declaring region as host with **no members** at
all — the record borrows nothing, where a callable borrows its home. Both bucket doors
([`OverloadSeal::of_delivered`](../src/machine/core/carrier_witness.rs) and its group sibling) and
the value wrapper ([`Scope::store_function_cell`](../src/machine/core/scope/reach.rs)) take that one
birth envelope as their operand — the seal rests it, the wrapper merges it — so the bound name and
the registered overload carry the same derived fact rather than two independent claims.

A deferred FN's per-call return *type* needs no residence machinery at all —
[`home_return_type`](../src/machine/core/kfunction/exec.rs) clones it into the captured-scope region
through the single type door — but the clone still comes back capped at the caller-supplied
**contract** lifetime rather than the captured region's own, so a `ret` reference cannot outlive the
window the lift boundary consumes it in. That cap is return-contract discipline, independent of
residence.

No runtime residence check survives. Each of the three region-borrowing families captures exactly one
region borrow (a `KFunction` its captured scope, a `Scope` its own region, a `Module` its child
scope), and none of the three can name a region the destination brand did not hand it.

None of the three needs a brand for *residence*, because nothing about them is erased on the
way in. A `KFunction` and a `Module` are `Copy` and store through the plain bump verb
([`BumpAllocator::value`](../workgraph/src/witnessed/bump.rs)) — the `KFunction`'s reaching it as the
placement's own bump inside its birth fold, which it takes for the composed reach rather than for
residence; a `Scope` is not `Copy` — it keeps
mutating in place through its `Cell` / `RefCell` fields — so it stores through
[`in_place`](../workgraph/src/witnessed/bump.rs), the glue-free verb whose `!needs_drop` assert
stands where the `Copy` bound stands for the others. Either way every reference the stored value
holds is an ordinary `&'a` the borrow checker already checked against the lifetime the destination
brand borrows its region for: no `'static` round trip to audit and so no residence obligation to
discharge.

The exception is a value embedding an operand from *another* region — the per-call frame child's
lexical parent, the transparent window's foreign binding table. Those two are **born at their
destination**: the value is constructed inside a `for<'b>` brand over the destination region and
bumped in the same act, through
[`RegionHandle::bump_born_with`](../workgraph/src/witnessed/region.rs). `'b` is universally
quantified with no outlives relation to any enclosing lifetime, so the only `&'b Region` the closure
body can name is the placement's own — an ambient region borrow does not coerce and does not compile.
That is the same no-outlives argument the folded sinks rest on. What leaves the brand is the bumped
**reference**, re-anchored to the caller's `'a` under the handle's own live region borrow; the value
itself is never erased. What each must still do is put its *own* bytes where its value
lands: a `KFunction`'s signature element run and re-homed name text
([`ExpressionSignature::mint`](../src/machine/model/types/signature.rs)), a `Module`'s path and both
member tables ([`Module::assemble`](../src/machine/model/values/module.rs)). Each takes a single
brand parameter for that re-home, so bumping the parts at one region and the value at another is
unstateable.

The koan doors ([`Scope::alloc_child_under`](../src/machine/core/scope.rs) and its same-region
siblings,
[`KFunction::alloc_captured`](../src/machine/core/kfunction.rs),
[`Module::alloc_at_child_scope`](../src/machine/model/values/module.rs)) each derive the destination
handle from the value's own anchoring scope rather than taking a brand alongside it, so a call site
cannot state a mis-pairing at all; the value-returning constructors are crate-internal, so none of
the three can exist outside the act that stores it. The frame-child door
([`build_frame_child_witnessed`](../src/machine/core/arena.rs)) is the one whose operand — the lexical
parent — genuinely lives in another region: it crosses the brand as a `SealedExtern` re-anchored to
the same `'b`, pinned by the frame's own `Rc<FrameStorage>`. That pin is borrowed for the destination
region's `'a`, so the witness contract covers the stored reference's whole life rather than the call,
and the parent-liveness chain stays typed by `CallFrame::new`.

Where a seam still has to *ask* where a composite lives — the copy-versus-pin decision's
home-crossing test — it reads the answer off the value:
[`ContainerSubstrate::homed_in`](../src/machine/model/values/container_substrate.rs) compares the
substrate's own stored reach description's host region by pointer. A region keeps no address table
at all — no membership vector, no per-family recording hook, no post-store side effect — so there is
nothing else it *could* consult; and there is no need, because the door that placed the substrate is
what made the stored host true.

A scheduler slot's scope handle is lifetime-free, so the node carries no `'run` through its scope.
A per-call frame scope is stored as a payload-less
[`NodeScope::Yoked`](../src/machine/execute/nodes.rs) marker re-projected from the slot's own
anchor cart (`SlotFrame.cart`); a genuinely run-lived scope (a binder body's decl-scope child) is stored
as `NodeScope::YokedChild`, a [`SealedExtern<ScopeRefFamily>`](../workgraph/src/witnessed.rs) carrier (a
`&'static Scope`) opened at read through the rank-2 `SealedExtern::open` at a `for<'b>` brand,
witnessed by the slot's cart `Rc`.
Both arms ride a grouped `NodePayload` (scope handle + lexical chain) *inside* the slot's memory anchor
(`SlotFrame`), which wraps the per-call cart the scheduler holds. The
slot-storage scope handle and the seed-side `with_scope` re-anchor are documented in
[per-call-region/scope-handles.md § Slot-table scope handle](per-call-region/scope-handles.md#slot-table-scope-handle).

### Debug region audits

Every rule above closes the **under-pinning** direction: a value naming storage
nothing keeps alive is unwritable. The other direction — a region pinned for longer
than any value reaching it needs — breaks no invariant, so it passes every one of
those checks silently. Two debug-only audits observe it. Both record and return:
neither changes what is retained, neither panics, and a release build compiles
neither.

The **pin-ring detector** is the library's, run at every region-lifetime retention
in any debug build, with the report surface re-exported from
[`witnessed`](../workgraph/src/witnessed.rs)
([reach.md § Debug audits](../workgraph/design/reach.md#debug-audits)). Koan needs
no wiring for it: `FrameStorage` is the library's `RegionHost`, so the walk's
ancestor enumeration is the same `outer` chain
[`pins_region`](../src/machine/core/arena/frame.rs) already answers over.

The **reach-tightness report** ([`reach_audit.rs`](../src/machine/core/reach_audit.rs))
is Koan's, and compiles only under `cfg(any(test, feature = "region-audit"))` —
`cargo run --features region-audit -- <program>` for an instrumented interpreter
run, whose findings [`main.rs`](../src/main.rs) prints to stderr afterwards. It
instruments one door, the fold chokepoint
[`StepAllocator::alloc_carried_with`](../src/machine/core/arena/step_allocator.rs),
and asks whether each declared operand actually reached the product.

Its ground truth is **address intersection, not stored reach**. Asking a value what
it borrows (`still_borrows`, a substrate's reach union) reads back what the folds
under audit declared — circular. The fold's own moment is not: inside the brand,
the addresses reachable from each operand view and from the product are collected,
and an operand *contributed* exactly when the two sets intersect. Only `usize`
addresses leave the brand, so the comparison runs outside it. A contributing
operand justifies its whole coverage — granularity is per operand, since a fold
embedding any part of an operand has no way to disclaim the rest of what that
operand reaches — and a description member justified by no contributor, and neither
the product's home nor eternal, is flagged. Pointer equality suffices for the
justification scan because a mint folds its sources' *exact* `Rc` members and
subsumption only ever drops members
([reach.md § Composition](../workgraph/design/reach.md#composition-minting-a-description-and-retaining-its-pins)).

What it does not see bounds how a clean run should be read: the direct
`merge_into` / `transfer_into` / `transfer_all_into` fold sites are not
instrumented, and the address walk records a `KFunction`'s or `Module`'s
captured-scope pointer without descending into that scope's binding tables, so a
product embedding a value reachable only through a captured binding reads as
non-contributing.

## Binding writes ride the step outcome

A builtin body never mutates a published scope's binding tables. It builds its value under the
step brand through one of the `seal_*` construction doors on
[`Scope`](../src/machine/core/scope/registry.rs) — mint the exact reach, copy the value in under
it, seal the two together — and returns the table write it decided as a
[`WriteOp`](../src/machine/core/bindings/ops.rs) on its
[`Action`](../src/machine/core/kfunction/action.rs). One variant per channel: `Value`
(a `data` entry — a value binding, callable by name alone), `Overload` (a
`FN` / `OP` dispatch-bucket entry, the only door a keyworded expression becomes dispatchable
through), `Type` (a `types` entry under a
[`TypeWritePolicy`](../src/machine/core/bindings/ops.rs)), `Group` (one operator-registry probe
key), and `SigSlot` (a `VAL` slot in the nearest enclosing SIG decl scope).

`run_action` deposits each interpreted `Action`'s effects into a harness-owned sink — a private
field on [`DecideCtx`](../src/machine/execute/decide/ctx.rs) with one deposit method the
execute layer alone can reach, so a builtin (which receives a `BodyCtx`) cannot touch it. The sink
is created per step by [`Host::step`](../src/machine/execute/harness.rs) and drained there, after
the step's continuation has returned and before its outcome is realized. `WriteOp::apply` is the
single interpreter: it writes against the step's own scope — which always owns its binding table,
since even a `USING` block runs in an owned layer stacked inside the borrowed window — runs the
builtin-shadow consult where the door asks for it, and mutates the table. Because nothing but the
step callback reaches this point, every map borrow is a firm `borrow_mut` — there is no koan frame on
the stack to hold a competing one, so contention is unrepresentable rather than tolerated.

Ops apply in `Vec` order, which is program order within the step. On the first failure the
remaining ops are dropped and the step becomes the node's error terminal, so the ordinary finalize
arms clear the producer's placeholders and attribute the error exactly as for an in-step error. A
body that errors before deciding its write installs nothing at all: the writes are outcome data,
and an error terminal carries none.

Consumers synchronize through the dep graph, not through write timing: a binder's placeholder and
pending-overload stamps go in at *submission* (already run-loop-owned — moving them later would let
a concurrent sibling see `UnboundName` instead of parking), and a parked reader wakes only at the
producer's finalize, which follows the drain position.

Writes into a scope no other node can reach need no such discipline and stay direct, through the
`*_direct` doors — which route through the same `WriteOp::apply` interpreter, so the table-write
rules exist once. Those are: startup builtin registration into the run-global root; parameter binds
and MATCH / TRY `it` into a not-yet-published per-call scope; a `GROUP` binder's registry seeding
into its own freshly minted child scope, before its body dispatches; an ascription view's bulk
install into a freshly minted view scope; and test fixtures.

## Structural invariants

Several "must hold" rules are encoded in types rather than checked at runtime:

- `Scope::region: &'a KoanRegion` is non-optional; `test_sink()` takes a
  caller-supplied region.
- `KFunction::captured_scope() -> &'a Scope<'a>` is non-optional.
- The running scope passes through `KoanRuntime::dispatch_in_scope(expr, scope)`
  directly, so dispatch sites carry their scope explicitly.
- [`KFunction::alloc_captured`](../src/machine/core/kfunction.rs) derives its destination region
  from the captured scope it is handed, so region-identity between a function and its captured
  scope is a type fact rather than a checked one — a misallocated `KFunction` is unwritable, not
  caught late as a use-after-free in `lift_kobject`'s fast path.

## Performance notes

The push/notify scheduler ([execution/README.md § Push/notify dependency
edges](../workgraph/design/dag-scheduler.md#pushnotify-dependency-edges)) keeps its slot-table
state in a
[`NodeStore`](../workgraph/src/scheduler/node_store.rs)
sub-struct that owns `slots: SlotVec<SlotState<W>>` (each slot a `PreRun(StoredWork)`
/ `Running` / `Free`) and `free_list: Vec<NodeId>`, behind the slot lifecycle
`alloc_slot → take_for_run → reinstall* → finalize`. `alloc_slot` is
the only path that picks an index (pulling from `free_list` before extending
`slots`), and `finalize` is the only path that ends a slot: it delivers the
terminal into each live edge's destination region and reclaims the slot the
moment its notify drains
([dag-scheduler.md § Delivery at finalize](../workgraph/design/dag-scheduler.md#delivery-at-finalize)).
Dependency bookkeeping lives alongside it in a
[`DepGraph`](../workgraph/src/scheduler/dep_graph.rs) sub-struct
that holds one `DepRow` per slot — `notify: Vec<EdgeId>` (this producer's
waiting edges) and `pending: usize` (this consumer's unfilled-inbound-edge
counter) — with the edges themselves in their own slab, each naming its
producer, its owner, and its destination region. Housing the row fields
together is what makes their coherence structural: the rows are private and
mutated only through `DepGraph`'s atomic-update methods, so the invariant
(every edge in a producer's `notify` matched by a +1 in `pending` on its
consumer) is enforced by the surface rather than by convention.

Transient-node reclamation is delivery itself: a slot reclaims at finalize the
moment its notify drains, so when a dispatch splice finish has rewritten
`working_expr.parts` to `WorkingPart::Spliced`, the spliced slots' indices are
back on the free-list as their producers deliver — the follow-on dispatch's
`alloc_node` recycles them without a separate release pass. A consumer's own
teardown releases the edges it holds; a producer whose consumers died before it
fired skips their released edges in its walk and reclaims all the same, so no
party's death schedule reaches into another's subtree.

## Verification

- [`a_rejected_binding_write_is_the_binders_error_terminal`](../src/machine/execute/harness/tests/statement_binder_install.rs)
  submits two colliding `OP` declarations as one block and confirms the second one's rejected
  bucket write surfaces on its own binder slot; its sibling
  `a_binder_that_errors_installs_nothing` confirms a body that errors before deciding its write
  leaves no binding behind.
- Per-call-region protocol verification (escaping-value relocation and retention, TCO
  frame reuse, MATCH `FrameStorage.outer` chain) is enumerated in
  [per-call-region/scope-handles.md § Verification](per-call-region/scope-handles.md#verification).
- [`region_death_frees_every_drop_free_family`](../src/machine/core/arena/tests.rs)
  fills one frame region with all five substrate shapes — each carrying a bumped string leaf, so
  the region holds re-homed bytes and index metadata as well as cells — plus a run of `KFunction`s
  whose signatures put a bumped element run and synthesized keyword / parameter-name bytes in the
  same region, plus a run of `Module`s whose paths, member-map keys and member-table bucket arrays land
  there too, and drops it with nothing borrowing in. That region death is *deallocation only* is a leak claim rather than a UB claim, and
  the only one a bump cannot fail loudly on: a family that reintroduced an owning slot would still
  read and write correctly and simply never free. Miri's process-exit leak count is the assertion;
  `Copy` at every bump primitive is the static proxy this checks in composition.
- [derived_reach.rs](../src/machine/core/tests/derived_reach.rs) takes each production registration
  door — the builtin seeds, `FN`, `OP` — and reads the description the bucket stores back off the
  opened carrier: it covers the callable's home region, names it a member (`borrows_home`), and
  covers no sibling region. Its group-record sibling asserts the other composed shape — hosted at
  the declaring region with no members — so both structural claims are checked as facts a
  composition produced rather than as prose.
- [`functor_application_mints_distinct_abstract_types`](../src/builtins/ascribe/tests/functor.rs)
  and [`a_returned_transparent_view_keeps_the_region_it_was_minted_in`](../src/builtins/ascribe/tests/ascription.rs)
  are the escaping-module half of the slate: an opaque view's path and both member maps are read
  back after the call region that bumped them is gone, and a transparent view — the one shape whose
  residence and the scope it borrows are different regions — is read back after its minting frame
  dies. A release claim derived from the borrowed child scope would free the storage those reads
  walk, which only tree borrows observes.
- The over-pinning audits ([§ Debug region audits](#debug-region-audits)) are
  observed by their own slates rather than trusted:
  [`over_fold_is_flagged`](../src/machine/core/reach_audit/tests.rs) drives a fold
  whose product embeds only its first operand through the real chokepoint door and
  reads back one flag naming the second operand's home, with three sibling tests
  fixing what must *not* flag (a tight fold, a co-homed non-contributor, a rebuilt
  scalar); [`mutual_pin_is_reported`](../workgraph/src/witnessed/tests/pin_cycles.rs)
  and its chain-mediated sibling build real rings — a genuine leak, so each test
  dismantles the ring it built — against two negatives that must stay silent.
- The audit slate runs cycle-free across every unsafe site in the runtime
  under `MIRIFLAGS=-Zmiri-tree-borrows` with zero UB and zero process-exit
  leaks, signing off the memory model as it stands today. The canonical
  slate list lives in [observe/miri_slate.md](../observe/miri_slate.md).

## Open work

- [Tightness-audit coverage](../roadmap/compile_safety/tightness-audit-coverage.md)
  — the two blind spots named under [§ Debug region audits](#debug-region-audits):
  the uninstrumented relocation verbs, and the address walk's stop at a captured
  scope's boundary.
