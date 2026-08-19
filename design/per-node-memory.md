# Per-node memory: Koan on the witnessed substrate

Every `KObject`, `Scope`, `KFunction`, … is born in a `KoanRegion` whose
sub-arenas store `T<'static>` and hand back a borrow re-anchored under a witness.
This doc owns **Koan's instantiation** of that machinery: which witness backs a
node, which construction verb each Koan site takes, where a bound value's reach is
minted, and how the run loop's reads nest under the substrate's access brand.

The generic machinery itself is the library's:
[workgraph/design/witnessed-memory.md](../workgraph/design/witnessed-memory.md)
owns the erase-store contract, the `Region<P>` allocator, the
`yoke` / `merge_into` / `map` construction surface, and the
`seal` / `open` / `transfer_into` access surface;
[workgraph/design/reach.md](../workgraph/design/reach.md) owns the reach
description / pin bundle representation and the holder rule.
[memory-model.md](memory-model.md) owns Koan's region/frame/lift mechanics and
[witness-hosting.md](witness-hosting.md) owns Koan's escape and residence policy
above the substrate.

## The Koan profile

`KoanRegion` is `Region<KoanStorageProfile>` over ten sub-arenas. The witness is
the per-call `Rc<FrameStorage>`, whose held `Rc` heap-pins the region for its life.
[arena.rs](../src/machine/core/arena.rs) holds only that profile
(`KoanStorageProfile`, `KoanRegion`, `FrameSet`, `CallFrame`) plus a thin
`RegionBrand` veneer over the library's
[`RegionHandle`](../workgraph/src/witnessed/region.rs) adding Koan-family-typed
`alloc_*` wrappers. The veneer carries no capability rule of its own; it allocates
through the generic engine via the `RegionOwner` seam — the `Rc<F>` blanket impl
that lets a foreign region-owner type pick up the library's region behaviour.

Allocation is reachable only through the branded `RegionBrand` handle: a bare
`&KoanRegion` exposes no `alloc_*`, so "always witnessed" is compile-enforced for
allocation.

## Which verb each construction site takes

**`yoke` — region-pure leaves.** An `alloc` site inverts so its construction runs
*inside* the closure: a region-pure leaf (`region.alloc_scalar(…)` over an owned
`Number` / `Bool` / `Null`) is a `yoke` whose closure is the single allocation.

A value embedding an AST — a quoted expression, an FN body — also `yoke`s, because
the embedded AST reaches no region a holder could outlive. Both are `Copy`
[`KExpression`](../src/machine/model/ast.rs) handles (the `KObject::KExpression`
and `Body::UserDefined` payloads) borrowing their whole content — parts run,
keyword text, structural cache — from the region that built them, and two
separate facts make that borrow harmless:

- **The value channel borrows program storage.** Every `KObject::KExpression` cell
  holds parsed AST, whose storage is the eternal-tier
  [`ProgramStorage`](../src/machine/core/arena/frame.rs) the parse door bumped it
  into. The eternal rule filters such a member out of every reach description
  ([value-substrates.md § Untyped arenas](value-substrates.md#untyped-arenas-the-drop-free-end-state)),
  so the cell reaches nothing. That tier is typed: the cell holds a
  [`ProgramExpression`](../src/machine/model/ast/program.rs), minted only through a
  [`ProgramBrand`](../src/machine/core/arena/frame.rs) door, so a node the runtime
  synthesizes at a per-call brand cannot enter the value channel at all
  ([value-substrates.md § Value-channel AST](value-substrates.md#value-channel-ast-the-program-storage-marker)).
  Shortening the brand into a step is what lets step code borrow long-lived data;
  the parts a door is handed are program-hosted by the door's own contract
  ([value-substrates.md § Value-channel AST](value-substrates.md#value-channel-ast-the-program-storage-marker)).
- **No expression names a producer region.** The per-call resolved sub-result the
  scheduler folds into a parent's parts lives on a different type —
  `WorkingPart::Spliced` on the scheduler's
  [`WorkingExpression`](../src/machine/model/ast/working.rs), which
  `KObject::KExpression` cannot hold — and an `FN` body is co-located with the
  `KFunction` that names it, so the function's own seed already covers it.

So the AST-embedding object is **region-pure**, born under the empty
(foreign-reach-only) set exactly as any region-pure leaf. The quote-capture site
takes its own door
([`RegionBrand::alloc_expression`](../src/machine/core/arena.rs), witnessed as
`alloc_expression_witnessed`) rather than the scalar one, for a lifetime reason
only: `KObject<'a>` is invariant, so a cell holding raw AST has no owned rebuild to
offer a lifetime-free signature. The door's own signature is the enforcement —
only a [`ProgramExpression`](../src/machine/model/ast/program.rs) reaches it, so the
node's parts run is program-storage hosted by type and the cell the door bumps
borrows nothing a seal would have to pin.

**`merge_into` / the relocation doors — everything that references a
pre-existing value.** An aggregate relocates its *element carriers* (deps
arriving witnessed from the lift) into its own region; a closure is *born*
through `merge_into`, its captured scope riding a resident seed operand
delivered at that very scope
([kfunction.rs](../src/machine/core/kfunction.rs)), so the callable's reach is
the fold's own composition. The object family's leaves and
aggregates are built this way — a single-part literal and a static aggregate cell
`yoke` their owned data, and a list / dict / record relocates its whole cell run
in one `transfer_all_into`
([decide/literal.rs](../src/machine/execute/decide/literal.rs) /
[single_poll.rs](../src/machine/execute/decide/single_poll.rs)). The
carrier-self-building constructions follow: the newtype / tagged-union
[`constructors`](../src/machine/execute/decide/constructors.rs) take the same
run door for a record newtype's fields and the pairwise `transfer_into` for the
single-value arms,
[`catch`](../src/builtins/catch.rs) relocates its one dep carrier, and FN def
[`finalize`](../src/builtins/fn_def/finalize.rs) hands the callable's birth
envelope to [`Scope::store_function_cell`](../src/machine/core/scope/reach.rs),
whose merge wraps it as a co-located `KObject::KFunction` under the description
that birth already composed.

The value-embedding sites that take a *bare arg* —
[`attr`](../src/builtins/attr.rs)'s `Wrapped`,
[`FROM`](../src/builtins/record_projection.rs)'s `Record`, and the
[literal.rs](../src/machine/execute/decide/literal.rs) Resolved arm's bound value
— climb off it the same way: each receives the value it embeds as a delivered
`Sealed` carrier and folds it into the result's own construction. `attr` / `FROM`
go through the step context's
[`alloc_carried_with`](../src/machine/core/arena.rs), which re-projects the value
at the fold brand from the lhs operand's own view (crossed via
[`BodyCtx::arg_carrier`](../src/machine/core/kfunction/action.rs)), so its reach
folds in by construction; the Resolved arm goes through the binding scope's own
[`Scope::seal_resident`](../src/machine/core/scope/reach.rs).

In Koan the cross-region case is rare. `merge_into` is the **same-region** case
almost always — a list assembled in one call's arena, or a closure capturing its
defining scope (a `KFunction` is allocated *into that scope's region*, so the
capture is co-located) — where subsumption trivially collapses the union to a
single `Rc`. The genuinely cross-region merges are *ancestry-related*: a scope or
function in a per-call frame referencing the run-global root or a lexical-ancestor
scope, where the descendant frame `Rc`'s `outer` chain already pins the ancestor
and subsumption drops it. The case that cannot collapse — a dep whose backing is an
independent, dying descendant region — is `transfer_into`, where the union is held
whole.

**The type family needs no reach.** A `KType` owns all its content, so a type
terminal instantiates the generic resident verb
([`Scope::resident`](../src/machine/core/scope/reach.rs)) with an empty,
pins-nothing witness, co-located with the region-resident type the slot write
([`WriteOp::SigSlot`](../src/machine/core/bindings/ops.rs)) installs. That holds
for a module self-sig (`KType::Signature { sig: SelfOf(m), .. }`) too — its
`SelfOf` names the module structurally, not by region pointer.

**No site pairs an already-built value with a separately-asserted witness.** The
region-pure carrier is built at the region's own handle
([`RegionHandle::seal_reaching`](../workgraph/src/witnessed/region.rs) under a
`mint_retained(&[])` description, and its delivered twin
[`RegionHandle::deliver_resident`](../workgraph/src/witnessed/region.rs)), which
mints a description hosted in that same region with **no members** — the value
records where it lives even though its borrows reach nothing. The residence, the
seal and (for the delivered form) the envelope's home pin all come off one handle,
so no call site pairs a value with a witness some other value derived; the
`'v: 'a` bound is the residence check, rejecting a borrow that does not outlive the
region handle. That emptiness is sound as a within-step transient, the producing frame folded
in at close ([`reseal_under`](../workgraph/src/witnessed.rs)) before the carrier is
stored. The transient is **typed**, not merely disciplined: the step doors return
the carrier wrapped as a
[`StepCarried`](../src/machine/execute/step_carried.rs) branded at the step's
`'step` lifetime, so the borrow checker rejects any attempt to stash it past its
construction step, and its sole exit to node storage is
[`StepCarried::seal_at_step`](../src/machine/execute/step_carried.rs) into
finalize's fold.

A node's own value terminal is witnessed the same way — a region-pure result (a
spliced value, a builtin's synchronous result) through `resident`, a dep-reaching
result by folding its delivered dep carriers — so the witnessed `Done` carrier is
the sole value
terminal. `seal_at_step` pairs it with the producing frame into a delivery
envelope, and [`finalize_terminal`](../src/machine/execute/finalize.rs) hands that
envelope on whole as `StepVerdict::Done(Ok(_))` — value and coverage stay one
value all the way to
the drain's finalize, whose delivery walk adopts it into each edge's
destination region. An error
carries no value and finalizes bare. The type / region construction operands are computed carriers
too — the newtype / tagged-union / `CATCH` build folds a delivered type-identity
carrier in as the destination operand under the binding's stored reach
([`build_type_operand`](../src/machine/execute/decide/constructors.rs)). A
declared return is checked and re-stamped in place in the producer's own region; no
relocation operand exists at Done.

A read of an *already-built* region-resident value — a bound name, an `ATTR` value
member, a defined FN object — does **not** rebuild a witness: it pre-exists its
carrier, so the read bundles it through the confined
[`RegionBrand::seal_resident`](../src/machine/core/arena.rs) surface, reached by the
one generic [`Scope::seal_resident`](../src/machine/core/scope/reach.rs) instantiated
per family, under the reach stored on its binding. `Witnessed::resident` is never
reached from a builtin, and no read walks a value to recover its reach.

## Which witness form each node takes

Koan uses both of the substrate's two sealed forms, and the split is not
incidental:

- A **node result** is self-witnessed under its producer frame `Rc`. The carrier is
  held *outside* the region it witnesses, so the strong `Rc` closes no cycle.
- The **per-call child scope** is externally-witnessed. It lives in the frame's own
  region, the `CallFrame` already holds the pinning `Rc`, and bundling a clone
  would peg `FrameStorage`'s refcount and defeat the `Rc::get_mut` uniqueness check
  TCO frame reuse depends on. The scope-pointer handle — an erased scope recovered
  against the frame `Rc` — *is* the externally-witnessed sealed carrier, rather than
  a scope-specialized erasure beside the substrate.

A value that *captures* the per-call scope therefore has no bundled scope witness
to merge against: it mints its merge operand from the frame `Rc` the
builder already holds — co-located, since the scope lives in that frame's region —
so the capturing carrier's minted reach gains that `Rc` and the escaping closure
pins the frame exactly as a node result does.

## Reach at the bind seam

A value *bound into a scope* has its reach **minted directly against the scope's
region** ([`Scope::adopt_for_binding`](../src/machine/core/scope/reach.rs)) — the
description into that region's reach table, the owned pins folded into that
region's one deduped union bundle. The binding entry itself owns **nothing**: it is
a `BindingIndex` beside a resting
[`SealedValue`](../src/machine/core/carrier_witness.rs), both `Copy` and
`Drop`-free. Because bindings are bind-once and a scope's entries never outlive its
region, entry death and region death are one schedule, so a region-owned union is
exactly as tight as a per-entry bundle and costs one `Rc` per distinct foreign
region instead of one per entry. A liveness-only retention (the FN return-type
slot, the `USING` overlay, the run-root drain) folds into the same union. There is
no scope-level reach-set and no deposit list to keep in sync; the adopt is the one
call that mints the description, retains the pins, and hands back what the binding
entry stores.

The bind sites adopt the bound value's full delivered carrier across both channels:
a [`let`](../src/builtins/let_binding.rs) adopts its bound value's carrier (an
object RHS or a resolved-type RHS alike), a user-fn arg bind adopts each argument
carrier — object and type — into the *per-call* scope at the frame bind itself
([`run_user_fn`](../src/machine/core/kfunction/exec.rs), the scope the parameters
bind on, reading each argument off the one envelope it was delivered in), and
[`USING`](../src/builtins/using_scope.rs)'s transparent window adopts
the opened module's carrier into the call-site scope it borrows into. A multi-region
value (a list of closures, a closure over several closures, a module reaching a
functor-result region) thus keeps *every* region it reaches alive for the life of
its scope's region.

The mint applies **no destination-relative narrowing**; only the library's self and
eternal rules shrink it, and Koan's eternal tier is the run root
([witness-hosting.md § The eternal tier](witness-hosting.md#the-eternal-tier)).

The minted reach rests **fused to its value** in one `Sealed` carrier, so a later
read hands the carrier back structurally and a value is never separated from the
reach that proves it. [`Bindings`](../src/machine/core/bindings.rs)' `data` entries
store that carrier — minted at bind time from the delivered carrier for a value or
alias, and composed from the child scope's own region for a module
([`Scope::store_module_object`](../src/machine/core/scope/reach.rs)), whose union
already covers everything its members reach. A value lookup or an `ATTR` member read hands
out a bit-copy of the seal — the thin description reference beside the value, no
bundle cloned — and the reader re-anchors it with no pin of its own
([`Scope::lift_resident`](../src/machine/core/scope/reach.rs) for travel,
`Sealed::open_at` for an in-step read, covered by the seal's own `'home` brand),
witnessing the existing `&'a KObject` **in place**.

The `types` channel stores no reach at all: a `KType` is a `Copy` handle interned
in the run frame's registry, so a bound type borrows nothing and names the same type
in every region. A type binding is a handle beside its
[`DeclarationSite`](../src/machine/core/bindings.rs), read out by copy; a bare type
leaf rides the resolve chain (`resolve_type_identifier`) with nothing to replay. A
freshly-built FN-def / LET-object seals one carrier through the confined resident
surface and the table write carries a duplicate of that same seal, so the bound
entry and the returned terminal share one allocation and one reach.

With both channels' construction carried and every bind's reach minted into its
scope's region, reach lives entirely on carriers: a value's reach is read off its
own carrier witness, never recovered by walking the value, and no scope-level
accumulator or deposit list exists to keep consistent alongside it.

## The step's coverage

Every re-anchor a step performs runs under one pin bundle: the **step's coverage**,
a [`FrameCoverage`](../src/machine/core/arena/frame.rs) built at step start over the
slot anchor's own region owner
([`Host::step`](../src/machine/execute/harness.rs)). A single `Rc` is the whole of
it, because that owner's `FrameStorage.outer` chain already pins every ancestor
backing a step can name.

Two things re-anchor at the step brand, and the coverage is what proves each of them
live:

- **The active scope**, which enters the open as its extern operand — so the
  coverage is exactly that operand's
  [`SealedExtern::open`](../workgraph/src/witnessed.rs) obligation, discharged once.
  A `Yoked` slot's scope lives in the anchor's own region; a `YokedChild`'s lives in
  an ancestor region the `outer` chain holds.
- **Every dep terminal.** A resolved dep is a resident cell the producer's finalize
  walk adopted into that edge's *destination* region, so the coverage has to pin that
  region rather than the producer's. It does, without enumerating anything: an owned
  dep's edge is destined at this slot's own anchor region
  ([`mint_source`](../src/machine/execute/harness.rs)), and a park's edge inherits
  the destination its source named
  ([`Scheduler::install_edge_from`](../workgraph/src/scheduler.rs)) — a claim edge
  destined at the scope introducing the name, which the consumer's lexical chain
  reaches and its `outer` chain therefore pins. Each cell re-brands **once** against
  the coverage ([`Retained::brand_with`](../workgraph/src/witnessed.rs)), and every
  read after that opens pin-free.

The continuation needs no coverage from here: its seal bundles a clone of the same
anchor at install ([`seal_work`](../workgraph/src/scheduler/nodes.rs)), so the owned
tier carries the liveness its own dormant captures read.

The bundle is assembled **before** the open and held **across** it, so it outlives
the step brand `'b` by construction — a carrier re-anchored to `'b` cannot outlive
the pin covering it, and the ordering is a local's scope rather than a rule to
remember.

## The run loop nests inside the access brand

The substrate's rank-2 brand forces the entire per-step consumption to nest inside
the closure. Koan pays that in full: the run-loop step nests its whole tail in one
brand — the continuation run, the outcome apply, and the finalize — through the
owned tier's single consuming `open` on the slot's `SealedPinned` continuation, so
nothing branded crosses the step boundary.

- **The dep slice** rides no carrier and enters the step `open` through no channel
  of its own: each [`DepTerminal`](../src/machine/core/kfunction/action.rs) is a
  resident cell of a region the step's coverage already pins, re-branded against it
  once at step start (above), so the slice is plain step-local data every reader
  opens pin-free (`Sealed::open_at`) at the borrow of the guard it binds. A
  construction finish that folds a dep into a longer-lived result lifts it back to a
  delivery envelope first
  ([`Scope::lift_spliced`](../src/machine/core/scope/reach.rs)), which owns the reach
  the fold composes.
- **The active scope** opens at that same brand: its carrier — the frame's own
  `SealedExtern<ScopeRefFamily>` for a `Yoked` slot, the node's own for a
  `YokedChild` — is zipped into the step `open` alongside the continuation, so the
  dispatch decide reads `&Scope<'b>` from the one brand (and the consumer `dest`
  region is the opened scope's own `region`, derived inside it) rather than
  re-anchoring a free `&Scope<'step>` up the dispatcher stack.
- **The run's program brand** enters by the brand bound rather than by channel.
  The step `open` bounds its brand above with a
  [`Within<'b, 'run>`](../workgraph/src/witnessed/dormant.rs) token whose declared
  `'run: 'b` the `for<'b>` instantiation discharges, which is what lets the
  [`ProgramBrand<'run>`](../src/machine/core/arena/frame.rs) the runtime holds be
  stored **unshortened** in the step's `DecideCtx`: that struct keeps
  `'program` distinct from `'step`, related only by its own `'program: 'step`
  bound, and the token discharges it. The brand is invariant, so it could not
  shorten in any case. It needs no seal, no re-anchor and no pin: it is a live
  borrow the compiler proves, and its region is eternal-tier, which every pin
  bundle filters out anyway.
- **Frame-side reads** fold onto `open` the same way: a frame's own child scope
  opens at a `for<'b>` brand through
  [`CallFrame::with_scope`](../src/machine/core/arena.rs) — the `&mut self` submit /
  classify paths reach it through `with_node_scope` / `with_current_node_scope`,
  copying out a scalar (an id, a region) where they need no live scope — so no
  `&Scope` rides up a `&mut self` path.
- **Seed-side binds** fold onto `open` too: the MATCH / TRY arm `it`-bind, the
  user-fn param-bind, and the deferred-return-type elaboration each open the child
  scope at the brand through `CallFrame::with_scope` and **relocate** their
  caller-`'a` value into the opened scope's own region through the substrate
  ([`Scope::adopt_for_binding`](../src/machine/core/scope/reach.rs), which relocates
  the value into the frame region at a fold brand, the composition minting and
  retaining what the copy still reaches rather than assuming purity — see
  [memory-model.md § Move-in residence](memory-model.md#move-in-residence)
  — for the `it` / param binds; the deferred return re-homing its elaborated `KType`
  into the captured-scope region) before binding it, so the value lands at the brand
  and the seed fabricates no free `&'a`.

Two driver accessors copy out inside the brand — a value read
([`read_result_with`](../workgraph/src/scheduler.rs)) and a borrow-free error probe
(`result_error`) — and the ride-up-stack dispatch sites resolve at the cart `'step`
directly. With every frame-side and seed-side read on `open`, the access surface is
`open` alone.

The construction-time scope re-anchor closes the same way: a same-region child
stores its already-`'a` parent by plain coercion, and the per-call frame child
builds through the externally-witnessed construction door
[`build_frame_child_witnessed`](../src/machine/core/arena.rs), which brands the
fresh region and the foreign parent at one `for<'b>` and erases the child
witness-less. No scope re-anchor survives outside the witnessed substrate.

## Storage choice, per node

The substrate is parametric over the witness and imposes no policy; Koan decides
per node. A user-fn call installs a fresh per-call region and witnesses its values
with that frame's `Rc`; a sub-expression node allocates into the *active* frame and
witnesses with the caller's pin; a tail-call chain reuses one node across a sequence
of fresh frames. This is why "per-node memory" names the carrier a node holds, not
an arena the node owns.
