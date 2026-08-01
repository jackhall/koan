# Per-node memory: Koan on the witnessed substrate

Every `KObject`, `Scope`, `KFunction`, … is born in a `KoanRegion` whose
sub-arenas store `T<'static>` and hand back a borrow re-anchored under a witness.
This doc owns **Koan's instantiation** of that machinery: which witness backs a
node, which construction verb each Koan site takes, where a bound value's reach is
minted, and how the run loop's reads nest under the substrate's access brand.

The generic machinery itself is the library's:
[workgraph/design/witnessed-memory.md](../workgraph/design/witnessed-memory.md)
owns the erase-store contract, the `Region<P>` allocator, the
`yoke` / `merge_pinned` / `map` construction surface, and the
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
*inside* the closure: a region-pure leaf (`region.alloc_object(…)` over owned or
region-derived parts) is a `yoke` whose closure is the single allocation.

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
  so the cell reaches nothing. The parse door takes a `ProgramBrand`, which types
  that tier; a node the runtime synthesizes mid-dispatch takes an ordinary
  `RegionBrand` and stays out of the value channel by flow rather than by type —
  the residual, stated in [`ProgramBrand`](../src/machine/core/arena/frame.rs)'s
  own doc and owned by
  [Typed expression value channel](../roadmap/compile_safety/expression-value-channel-guard.md).
- **No expression names a producer region.** The per-call resolved sub-result the
  scheduler folds into a parent's parts lives on a different type —
  `WorkingPart::Spliced` on the scheduler's
  [`WorkingExpression`](../src/machine/model/ast/working.rs), which
  `KObject::KExpression` cannot hold — and an `FN` body is co-located with the
  `KFunction` that names it, so the function's own seed already covers it.

So the AST-embedding object is **region-pure**, born under the empty
(foreign-reach-only) set exactly as any region-pure leaf. The quote-capture site
still takes the audited door
([`RegionBrand::alloc_object_witnessed_checked`](../src/machine/core/arena.rs))
rather than `alloc_object_witnessed`, but for a lifetime reason only: `KObject<'a>`
is invariant, so a cell holding raw AST has no `'static` rebuild to offer the
unchecked signature. Its residence walk has nothing left to reject.

**`merge_pinned` / `transfer_into` — everything that references a pre-existing
value.** An aggregate folds its *element carriers* (deps arriving witnessed from
the lift) via `transfer_into`; a closure folds the captured-scope operand minted
from its frame `Rc` via `merge_pinned` directly. The object family's leaves and
aggregates are built this way — a single-part literal and a static aggregate cell
`yoke` their owned data, and a list / dict / record folds its dep carriers via
`transfer_into` ([dispatch/literal.rs](../src/machine/execute/dispatch/literal.rs) /
[single_poll.rs](../src/machine/execute/dispatch/single_poll.rs)). The
carrier-self-building constructions follow: the newtype / tagged-union
[`constructors`](../src/machine/execute/dispatch/constructors.rs) and
[`catch`](../src/builtins/catch.rs) fold their dep carriers, and FN def
[`finalize`](../src/builtins/fn_def/finalize.rs) `yoke`s its co-located
`KObject::KFunction` onto a carrier witnessed by the defining scope's frame.

The value-embedding sites that take a *bare arg* —
[`attr`](../src/builtins/attr.rs)'s `Wrapped`,
[`FROM`](../src/builtins/record_projection.rs)'s `Record`, and the
[literal.rs](../src/machine/execute/dispatch/literal.rs) Resolved arm's bound value
— climb off it the same way: each receives the value it embeds as a delivered
`Sealed` carrier and folds it into the result's own construction. `attr` / `FROM`
go through the step context's
[`alloc_carried_with`](../src/machine/core/arena.rs), which re-projects the value
at the fold brand from the lhs operand's own view (crossed via
[`BodyCtx::arg_carrier`](../src/machine/core/kfunction/action.rs)), so its reach
folds in by construction; the Resolved arm goes through the binding scope's own
[`Scope::seal_resident`](../src/machine/core/scope/reach.rs).

In Koan the cross-region case is rare. `merge_pinned` is the **same-region** case
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
region-pure carrier is built by
[`Witnessed::resident_in`](../workgraph/src/witnessed/carrier.rs), which mints a
description hosted in the named home region with **no members** — the value records
where it lives even though its borrows reach nothing — so it cannot pair a value
with a *wrong* witness, only with the empty reach a region-pure value genuinely
has. That emptiness is sound as a within-step transient, the producing frame folded
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
result by folding its delivered dep carriers — so
[`NodeStep::DoneWitnessed`](../src/machine/execute/nodes.rs) is the sole value
terminal and [`finalize_terminal`](../src/machine/execute/finalize.rs) folds the
producing frame into that carrier's own reach at close. An error carries no value
and finalizes bare. The type / region construction operands are computed carriers
too — the newtype / tagged-union / `CATCH` build `merge_pinned`s a delivered
type-identity carrier under the binding's stored reach
([`build_type_operand`](../src/machine/execute/dispatch/constructors.rs)). A
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
to `merge_pinned` against: it mints its merge operand from the frame `Rc` the
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
carrier — object and type — into the *per-call* scope
([`exec::invoke`](../src/machine/execute/dispatch/exec.rs), the scope the parameters
bind on), and [`USING`](../src/builtins/using_scope.rs)'s transparent window adopts
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
alias, and from the child scope's own region for a module
([`Scope::child_module_reach`](../src/machine/core/scope.rs)), whose union already
covers everything its members reach. A value lookup or an `ATTR` member read hands
out a bit-copy of the seal — the thin description reference beside the value, no
bundle cloned — and the reader re-anchors it under a pin it already holds
([`Scope::lift_resident`](../src/machine/core/scope/reach.rs) for travel,
`Sealed::open_at` for an in-step read), witnessing the existing `&'a KObject` **in
place**.

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

## The run loop nests inside the access brand

The substrate's rank-2 brand forces the entire per-step consumption to nest inside
the closure. Koan pays that in full: the run-loop step nests its whole tail in one
brand — the continuation run, the outcome apply, and the finalize — through the
consuming externally-witnessed `open`, so nothing branded crosses the step
boundary.

- **The dep slice** opens *in-band* at that same brand: each producer terminal is
  read out borrow-bounded, erased into one slice carrier, and zipped alongside the
  continuation, so every dep value is born at `'b` through the one step `open` with
  no separate slice reattach.
- **The active scope** opens at that same brand: its carrier — the frame's own
  `SealedExtern<ScopeRefFamily>` for a `Yoked` slot, the node's own for a
  `YokedChild` — is zipped into the step `open` alongside the continuation, so the
  dispatch decide reads `&Scope<'b>` from the one brand (and the consumer `dest`
  region is the opened scope's own `region`, derived inside it) rather than
  re-anchoring a free `&Scope<'step>` up the dispatcher stack.
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
  ([`Scope::store_object_adopted`](../src/machine/core/arena/residence.rs), which
  re-homes the value at the frame region under a residence audit against the bind's
  own reach evidence rather than assuming purity — see
  [memory-model.md § Move-in residence audits](memory-model.md#move-in-residence-audits)
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
