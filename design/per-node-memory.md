# Per-node memory: the witnessed substrate

A scheduler node is a long-lived object that nevertheless eventually dies, and
between birth and death it must hold values that *borrow* from memory it does not
own. The `witnessed` substrate is the generic, workload-independent machinery that
makes those borrow-carrying values safe to store, move, and read across a node's
life — a bump allocator, a liveness witness, and a small carrier surface, naming
no Koan type. `machine` and `scheduler` both depend on it; it depends on neither.

The design goal is a single safe interface over per-node memory: every access is a
borrow the compiler checks, and the substrate's own `unsafe` is confined to a
handful of audited lifetime retypes no caller can reach.

## The core: erase-store, witness, reattach

**Generic.** A value of type `T<'a>` cannot be stored in a structure that outlives
`'a`. The substrate stores its `'static`-erased form `T<'static>` instead — sound
because a lifetime is zero-sized, so `T<'a>` and `T<'static>` share layout — and
re-anchors a borrow on the way out. Three pieces carry the contract:

- `Reattachable` — an `unsafe` trait marking a family `{ type At<'r>; }` whose
  representation is identical across every choice of its one lifetime. `Erased<T>`
  stores `T::At<'static>`.
- `Witness` — an `unsafe` marker asserting its holder pins the value's backing at a
  fixed address while held (`StableDeref`-backed). A re-anchor is sound only while
  a witness is held.
- A single private `retype<A, B>` lifetime-cast, guarded by a size assert, is the
  only place a `T::At<'a> → T::At<'b>` retype is written. Every accessor routes it.

**In Koan.** Every `KObject`, `Scope`, `KFunction`, … is born in a `KoanRegion`
whose sub-arenas store `T<'static>`. The witness is the per-call `Rc<FrameStorage>`,
whose held `Rc` heap-pins the region for its life. The region/frame/lift mechanics
are owned by [memory-model.md](memory-model.md); this doc owns the substrate the
mechanics instantiate.

## The bump allocator

**Generic.** `Region<P>` is the erase-store engine: a set of typed sub-arenas
parameterized by a storage profile `P`, holding `At<'static>` and handing back an
`&'a` tied to the caller's input borrow. It keeps the typed-arena Drop discipline —
each stored value's `Drop` runs, and touches only owned contents, never a
lifetime-parameterized reference (sub-arenas drop together, so any cross-arena `&`
is dead before it could be observed). This is what makes a byte-bump allocator that
forgoes Drop (`bumpalo`) the wrong fit: the Drop discipline *is* the soundness
argument, and dropping it would mean re-proving every stored type leak-free by hand.

**In Koan.** `KoanRegion` is `Region<KoanStorageProfile>` over seven sub-arenas. The
witnessed allocation surface is the substrate's `alloc`, which — see *Construction* —
hands the freshly-stored value to a `for<'b>` brand and returns it already wrapped in
its carrier, witnessed by the value's *foreign* reach (the active frame folded in only
at close), so a region-resident value is born inside the Witnessed/Sealed abstraction
rather than handed out as a bare `&'a` and re-wrapped downstream.

## Construction: `yoke`, `merge_pinned`, `map`, and one wrapper per node

The carrier `Witnessed<T, W>` bundles `Erased<T>` with the witness `W` that pins it,
so "the witness keeps the value alive" is a type invariant, not a co-stored pair
plus a comment. Three constructors build it; their division of labour is the heart
of the design.

**`yoke` — mint a value into a region.** Generic: `yoke` hands the witness's own
region to a rank-2 `for<'b> FnOnce(&'b Region) -> T::At<'b>` closure and bundles
whatever it builds. Because the closure is universally quantified in `'b`, it cannot
return a reference captured from its environment (a foreign `&'x` would need
`'x: 'b` for every `'b`) — so the produced value's references are *region-derived or
owned*, and co-location (the witness pins *this* value's references) holds by
construction rather than by assertion. The witness enters here, as a parameter,
because there is no prior carrier to inherit it from: `yoke` is the door through
which a value first becomes witnessed. In Koan: an `alloc` site inverts so its
construction runs *inside* the closure — a region-pure leaf
(`region.alloc_object(…)` over owned or region-derived parts) is a `yoke` whose
closure is the single allocation. A value embedding an AST — a quoted expression, an FN body — also
`yoke`s, because the embedded AST is *owned data*, not a borrow. An FN body and a quoted expression are owned
[`KExpression`](../src/machine/model/ast.rs) clones (the `KObject::KExpression` and
`Body::UserDefined` payloads). A `KExpression`'s lifetime parameter names no live borrow: its one
non-owned variant — `ExpressionPart::Spliced`, the per-call resolved sub-result the scheduler folds
into a parent's parts — holds a lifetime-free `Sealed` carrier cell, so `'a` is a phantom across
every `KExpression`, re-anchored invariantly by a zero-size `PhantomData` marker. Raw, unevaluated
AST is additionally *splice-free*: it holds no `Spliced` cell at all. `KExpression` is therefore a
[layout-invariant carrier family](#the-core-erase-store-witness-reattach) — the splice cell is
lifetime-free and the marker zero-size, so the layout is identical across `'a` — and
a splice-free embed contributes no foreign region: the AST-embedding object is **region-pure** and
allocs through the witnessed object surface (`alloc_object_witnessed`), born under the empty
(foreign-reach-only) set exactly as any region-pure leaf. Co-location is enforced by the `for<'b>`
brand; the embedded AST contributes no region of its own, and the sole residual obligation — that the
embed is splice-free — is a `debug_assert` at the `QUOTE` site, not a witness the type encodes.

The witness `yoke` takes is a *single-region* type — a lone region owner — so a mint pins exactly one
region by construction, not by narrowing a set that might be empty or hold several; a minted leaf then
lifts to the **reference-only** carrier the aggregate stores through a distinct
[`into_reference_only`](../workgraph/src/witnessed.rs) lift — its own region kept alive externally,
by containment or the retention hold, so the carrier holds no pin — kept separate from `yoke` so
minting stays a one-region act and combining regions (minting the reach set into the destination
arena) stays `merge_pinned`'s job. What `yoke` cannot mint composes through `merge_pinned` (or the
`transfer_into` it shares its `ComposeWitness::compose` engine with, for a dep-delivered operand): an
aggregate folds its *element carriers* (deps arriving witnessed from the lift) via `transfer_into`; a
closure folds the captured-scope operand minted from its frame `Rc` via `merge_pinned` directly. The
object family's region-pure leaves and aggregates are built this way — a single-part
literal and a static aggregate cell `yoke` their owned data, and a list / dict / record folds its
dep carriers via `transfer_into` ([dispatch/literal.rs](../src/machine/execute/dispatch/literal.rs)
/ [single_poll.rs](../src/machine/execute/dispatch/single_poll.rs)) — co-location the `for<'b>`
brand enforces, never asserted. The carrier-self-building object constructions are built the same way:
the newtype / tagged-union [`constructors`](../src/machine/execute/dispatch/constructors.rs)
and [`catch`](../src/builtins/catch.rs) fold their dep carriers via `transfer_into` / `merge_pinned`, and FN
def [`finalize`](../src/builtins/fn_def/finalize.rs) `yoke`s its co-located `KObject::KFunction` onto a
carrier witnessed by the defining scope's frame. The value-embedding sites that take a *bare arg* —
[`attr`](../src/builtins/attr.rs)'s `Wrapped`, [`FROM`](../src/builtins/record_projection.rs)'s
`Record`, and the [literal.rs](../src/machine/execute/dispatch/literal.rs) Resolved arm's bound value —
climb off it the same way: each receives the value it embeds as a delivered
[`Sealed`](#storage-and-access-seal-open-transfer_into) carrier and folds it into the result's own
construction — `attr` / `FROM` through the step context's
[`alloc_object_with`/`alloc_type_with`](../src/machine/core/arena.rs) (the dep's reach folded in at the
same alloc site that builds the value, via [`BodyCtx::arg_carrier`](../src/machine/core/kfunction/action.rs)),
the Resolved arm through the binding scope's own
[`resident_value_carrier`](../src/machine/core/scope.rs) — so the projected object
names every region it reaches by construction. No construction terminal pairs an *already-built* value
with a separately-asserted witness: the **type** family seals the same way — a region-pure or owned `KType`
through the step context's [`alloc_type_with`](../src/machine/core/arena.rs), a region-referencing `KType::Module` through
[`Scope::resident_type_carrier`](../src/machine/core/scope.rs) under the child-scope reach folded at
construction ([`Scope::reach_of_child`](../src/machine/core/scope.rs), from the child scope the birth
site holds directly) — so every multi-dep constructed value, object or type, is born co-located by the
`yoke` brand. The region-pure
carrier is built by the purpose-built [`Witnessed::resident`](../workgraph/src/witnessed.rs), which fixes the
witness to `W::default()` — the empty, pins-nothing set — so it cannot pair a value with a *wrong*
witness, only with the empty reach a region-pure value genuinely has; that emptiness is sound as a
within-step transient, the producing frame folded in at close ([`reseal_under`](../workgraph/src/witnessed.rs))
before the carrier is stored. A node's own value terminal is witnessed the same way — a region-pure
result (a spliced value, a builtin's synchronous result) through `resident`, a dep-reaching result by
folding its delivered dep carriers — so [`NodeStep::DoneWitnessed`](../src/machine/execute/nodes.rs) is
the sole value terminal and [`finalize_terminal`](../src/machine/execute/finalize.rs) folds the
producing frame into that carrier's own reach at close rather than asserting a separately-computed
witness set; an error carries no value and finalizes bare. The type / region construction operands are
computed carriers too — the newtype / tagged-union / `CATCH` build `merge_pinned`s a delivered type-identity
carrier under the binding's stored reach ([`build_type_operand`](../src/machine/execute/dispatch/constructors.rs)),
the contract-home operand is born region-pure via `resident` and folded by `merge_pinned`, and the relocate
destination rides a `yoke`d-or-`resident` carrier — so no site pairs an already-built value with a
separately-asserted witness. A read of an
*already-built* region-resident value — a bound name, an `ATTR` value member, a defined FN object —
does **not** rebuild a witness: it pre-exists its carrier, so the read bundles it through the confined
[`RegionBrand::seal_resident`](../src/machine/core/arena.rs) surface
([`Scope::resident_value_carrier`](../src/machine/core/scope.rs) / `resident_type_carrier`) under the
reach stored on its binding (see [Storage and access](#storage-and-access-seal-open-transfer_into)), so
`Witnessed::resident` is never reached from a builtin and no read walks a value to recover its reach.

**`merge_pinned` — fold many region-resident values into one.** Generic: a value built
from references into *two* regions cannot be bundled with one witness by `yoke`
alone. `merge_pinned` re-anchors two carriers at one shared brand under an **externally
supplied pin** covering the source (`self`) operand's backing, runs a projection that
binds one into the other, and re-seals under the **composed** witness — the union of
the two operands' regions, with `outer`-chain subsumption dropping a region another
already pins. The composition is `ComposeWitness::compose`, run inside the shared
brand with the destination in scope: an owned region *set* composes by plain union
(total, since a set can always represent the combined pin), while a hosted carrier
mints the union into the destination's own arena. This is what keeps
witnessed-ness at the *boundary*: without it, an aggregate of independently-witnessed
elements would nest `Witnessed<…Witnessed<…>>` wrappers with the data and be
unstorable as a single node carrier. With it, the invariant holds:

> **One wrapper per node.** A node stores exactly one carrier, regardless of value
> complexity. `yoke` mints leaves into a region; `merge_pinned` folds region-resident
> values — same-region or cross-region — into one aggregate under the single witness
> that pins them all; the result seals as one unit. Wrapper count is O(1) per node,
> not O(data size).

In Koan, `merge_pinned`'s trigger is *referencing a pre-existing region-resident value* — the
foreign borrow a `yoke` closure would reject — and it is the **same-region** case almost
always: a list assembled in one call's arena, or a closure capturing its defining scope (a
`KFunction` is allocated *into that scope's region*, so the capture is co-located), where
subsumption trivially collapses the union to a single `Rc`. The genuinely
cross-region merges are *ancestry-related* — a scope or function in a per-call frame
referencing the run-global root (or a lexical-ancestor scope) — where the descendant frame
`Rc`'s `outer` chain already pins the ancestor region, so subsumption keeps the frame witness
and drops the ancestor's. The case `merge_pinned` *cannot* collapse — a value whose backing reaches an
**independent, dying** region — is `transfer_into` (below) instead: there the source is a dying
*descendant*, so subsumption would collapse onto the backing about to drop; the union must be held
*whole* as the set of both.

**`map` — advance a value already witnessed.** Generic: `map` consumes a carrier,
re-anchors it at a brand, transforms `T::At<'b> → P::At<'b>`, and re-seals under the
*same* witness. It differs from `yoke` in source (an existing carrier, not a region)
and from `with` (below) in that the brand-flavoured result is *kept* — re-sealed —
rather than forbidden from escaping. In Koan: stepping a witnessed continuation to
its next witnessed state without changing which cart pins it.

## Storage and access: `seal`, `open`, `transfer_into`

A node holds its carrier *between* run-loop steps, when nothing is being read. The
access surface models exactly that rhythm.

**Sealing.** Generic: `seal` turns the live `Witnessed<T, W>` into a `Sealed<T, W>`
— the node-storage form, opaque between accesses, exposing no construction or
transform. Sealing is the same operation that lifts a finalized result into a slot:
bundle the erased value with the witness that pins it. In Koan: `finalize` sealing a
node's terminal under its producer frame `Rc`.

**Two witness forms.** Generic: a sealed carrier comes in two shapes, distinguished
by where the witness lives. The **self-witnessed** form bundles `W` (the
`Sealed<T, W>` above): for a value *minted* into a fresh region whose pin nothing
else holds. The **externally-witnessed** form carries *no* bundled witness; the
holder already pins the backing and supplies it at the access, read through a
**consuming, externally-witnessed `open`** — the witness handed in at the call and the
carrier moved into the same rank-2 `for<'b>` brand, so a non-`Copy` carrier (a continuation)
passes and nothing branded escapes. Bundling a witness the carrier does
not need would be a redundant second owner — and, when the witness is
reference-counted, an extra count the holder's own uniqueness checks must subtract.
`yoke`, which moves `W` into the bundle, builds the self-witnessed form; the
externally-witnessed form is built with the witness-less `erase` and read against an
external pin. In Koan: a node result is self-witnessed under its producer frame `Rc`;
the per-call child scope is externally-witnessed — it lives in the frame's own
region, the `CallFrame` already holds the pinning `Rc`, and bundling a clone would
peg `FrameStorage`'s refcount and defeat the `Rc::get_mut` uniqueness check TCO frame
reuse depends on. So the scope-pointer handle — an erased scope recovered against the
frame `Rc` — *is* the externally-witnessed sealed carrier, and collapses into this
one substrate rather than a scope-specialized erasure.

This split is what keeps self-witnessing cycle-free. A self-witnessed carrier's strong frame
`Rc` rides the *carrier*, which a node holds *outside* the region it witnesses; `merge_pinned` folds
every intermediate into that one carrier (the *one wrapper per node* invariant above), so no
region-resident value strong-owns its own frame — the value in-region holds only non-owning
pointers (a plain `&Scope`, a `Weak` `region_owner`). The per-call scope is the one value held
*inside* the frame, which is exactly why it stays externally-witnessed. A value that *captures*
the scope therefore has no bundled scope witness to `merge_pinned` against: it mints its merge operand
from the frame `Rc` the builder already holds — co-located, since the scope lives in that frame's
region — so the capturing carrier's witness set gains that `Rc` and the escaping closure pins the
frame exactly as a node result does.

**Opening.** Generic: `open` is the access verb — a rank-2
`open<R>(&self, for<'b> FnOnce(Live<'b, T>) -> R) -> R` for the self-witnessed form, with a
consuming, witness-supplied twin for the externally-witnessed form (same brand, witness handed in
at the call rather than bundled). Between calls the carrier is
`Erased`: no live reference exists. Each `open` is a borrow-scoped window in which
references go live, branded `'b`; `R` cannot name `'b`, so nothing branded escapes
the window. This is the design's safety core, and the RAII analogy is exact: *behave
like RAII while accessed — borrow-checked, references confined — but instead of
dropping, go opaque until the next access.* No `'b`, no access; a value that must
outlive the window leaves it only as an owned copy or by transfer.

**Transfer.** Generic: `transfer_into` is the safe relocation — it moves the sealed
value into a *consumer's* storage at the destination's lifetime, keeping every region
the value still reaches alive by holding that region's frame `Rc`. Copying is not an
option: a captured closure may reference anything reachable from its scope, and a
region carries no per-value reachability map, so the source regions are *kept*, not
rebuilt. The carrier is therefore witnessed by the **set** of regions the value
reaches — the destination it was relocated into, plus each source region a retained
closure still borrows. These regions form a tree, not a chain — a closure capturing
closures branches into independent lineages — flattened into the set; a value with no
cross-region reference is the degenerate singleton (the destination alone). This is **not** a `merge_pinned`: the source is a dying
*descendant* of the destination (its ancestry pins the destination, not the reverse),
so no single dominating witness exists — the set is held whole and composed by union,
since splicing the source into the destination's `outer` chain to collapse it risks
re-forming the `src`↔`dst` cycle. This closes the one case `open` cannot: a value
whose source backing is dying but whose consumer outlives it. In Koan: the
consumer-pull lift across a dependency edge — `copy_carried` copies the dep into the consumer
`dest` region at the step brand (the spine sharing its `Rc` payloads, a closure / future / module
riding its bare borrow), and `transfer_into` re-seals it under the set union of its reached sources
and `dest`. The lift delivers each dep **both** as a live bare `Carried` and as its producer slot's
own `Sealed` carrier (a `duplicate`): a finish that embeds or binds a value folds that carrier so the
reach is named on the carrier and never reconstructed. Every object construction does this — the
aggregate and region-pure inversions, the newtype / tagged-union constructors, and `catch` fold their
dep carriers via `transfer_into`; the bare-arg value-embedding sites (`attr`, `FROM`, the literal
Resolved arm) `transfer_into` the [delivered carrier](#storage-and-access-seal-open-transfer_into) of the value
they project; and a `let` or user-fn arg bind mints the bound value's carrier into the scope's own arena
(below). The **type** channel rides the same construction: a region-pure or owned `KType` seals via
the step context's [`alloc_type_with`](../src/machine/core/arena.rs), which folds its own region (the
producer's home frame) plus any listed dep's reach at the alloc site itself; a
region-referencing `KType::Module` seals via [`Scope::resident_type_carrier`](../src/machine/core/scope.rs)
under the child-scope reach minted once at construction from the child scope the birth site holds
directly ([`Scope::reach_of_child`](../src/machine/core/scope.rs)), never recovered by walking the built
`KType::Module`. A relocated module therefore names every region it reaches on its own witness, read
back at the consumer rather than reconstructed from the value. No finish reads a live
value out to rebuild its reach: the relocate-into-consumer seam is a plain
[`copy_carried`](../src/machine/execute/lift.rs) structural copy, transient reach rides each dep's
carrier, and only a *bound* value mints into the scope's own arena (below).

A value *bound into a scope* has its reach **minted directly into the scope's own arena**
([`Scope::host_reach_of`](../src/machine/core/scope.rs)), producing a resident `{ bit, ref }` binding
entry rather than a separate scope-level accumulator: the mint is held by the arena for the scope's
region's life — the same schedule the scope itself is held on — so discarding the returned reference
after a liveness-only mint (the FN return-type slot, the `USING` overlay, the run-root drain) is sound,
and a bind that produces an entry stores the reference on it. There is no scope-level reach-set and no
deposit list to keep in sync; the mint is the one call that both pins the reach for the scope's life and
hands back what the binding entry stores. The bind sites mint from the bound value's full delivered
carrier across both channels: a [`let`](../src/builtins/let_binding.rs) mints its bound value's carrier
(an object RHS or a resolved-type RHS alike), a user-fn arg bind mints each argument carrier — object
and type — into the *per-call* scope ([`exec::invoke`](../src/machine/execute/dispatch/exec.rs), the
scope the parameters bind on), and [`USING`](../src/builtins/using_scope.rs)'s transparent window mints
the opened module's carrier into the call-site scope it borrows into. A multi-region value (a list of
closures, a closure over several closures, a module reaching a functor-result region) thus pins *every*
region it reaches for the life of the scope that outlives it.

[`Scope::host_reach_of`](../src/machine/core/scope.rs)'s mint **omits ancestor regions** the scope
already keeps alive: its own / a storage-`outer` ancestor ([`FrameStorage::pins_region`]), *and* a
**lexical** `outer`-chain ancestor. The lexical omission is load-bearing under TCO: a per-call frame
carries no storage `outer` link, so the storage walk stops at its own region while a captured closure
still pins its defining (lexical-ancestor) scope. Minting such an ancestor into the arena — paired with
a sibling bind of the call's result — would close a `region → scope → arena → frame` cycle and defeat
the `Rc::get_mut` TCO frame-reuse gate; omitting it realizes
[`fold_omitting`](#construction-yoke-merge_pinned-map-and-one-wrapper-per-node)'s "omit ancestors" intent while
keeping a region-pure or ancestor-bound value pinning nothing (the mint returns `None`). A value
adopted into a scope arrives as its delivery envelope; the bind mints its reach — and, at
`Residence::Kept`, the producer's host frame — into the scope's own arena
([`Scope::adopt_sealed`](../src/machine/core/scope.rs)), so the resident binding entry names
everything the value reaches under the scope's own liveness, and the reference-only carrier it stores
holds no pin into a producer frame.

The minted reach is stored **per binding**, so a later read hands its carrier back structurally.
[`Bindings`](../src/machine/core/bindings.rs)' `data` and `types` entries each carry the bound value's
home-omitted foreign `Option<&FrameSet>` alongside the reference — minted at bind time from the
delivered carrier for a value or alias ([`Scope::host_reach_of`](../src/machine/core/scope.rs)), and
minted from the child scope's binding-entry reaches, held directly at construction, for a module
([`Scope::reach_of_child`](../src/machine/core/scope.rs)). A carrier-oriented lookup
(`lookup_value_carrier` / `lookup_type_carrier`) or an `ATTR` member read hands that stored reach back —
copying the thin reference, never cloning the set — and the read builds a self-contained terminal —
home frame fetched fresh, ∪ the stored foreign reach — through
[`Scope::resident_value_carrier`](../src/machine/core/scope.rs) / `resident_type_carrier`, witnessing
the existing `&'a KObject` / `&'a KType` **in place**. A bare type leaf rides the reach through the
whole resolve chain (the `type_identifier_memo` and `resolve_type_identifier`), recomputing it at the
memo miss by name ([`Scope::resolve_type_reach`](../src/machine/core/scope.rs)). The stored reach is
home-omitted for the same cycle-safety rule every mint obeys — the region's own home frame `Rc` never
lands in-region, so no `frame → region → scope → bindings → frame` strong cycle forms. A freshly-built
FN-def / LET-object registers its reference through the scope's frame-lifetime `&'a` and seals only its
*terminal* carrier through the confined resident surface, so the registered reference and the returned
carrier share one allocation.

With both channels' construction carried, binds minted, and each binding's reach stored, reach lives
entirely on the node carrier and — for bindings — on each binding's own minted reference: a value's
reach is read off its own carrier witness or its stored reach, never recovered by walking the value, and
no scope-level accumulator or deposit list exists to keep consistent alongside it.

## Why reads are safe

The danger in any reattach is a *free, unbounded* content lifetime the caller can
widen past the witness pin — the Miri-proven use-after-free the naive content-free
reattach exhibits. `open`'s rank-2 brand forecloses it: the fabricated lifetime is
universally quantified and un-nameable, so it cannot be widened or captured. Reads
therefore lose no safety — a reference may escape the *call* (the value drives the
step's work), but only as an owned copy or pin-bounded transfer, never as a branded
borrow outliving its window.

The rank-2 brand forces the entire per-step consumption to nest inside the closure; where a
re-anchored reference would otherwise ride up the dispatcher call stack, that becomes either
copy-out or a CPS rewrite of the step. The run-loop step nests its whole tail this way — the
continuation run, the outcome apply, and the finalize all run inside one brand (the consuming
externally-witnessed `open` above), so nothing branded crosses the step boundary. The step's dep
slice is opened *in-band* at that same brand — each producer terminal read out borrow-bounded, erased
into one slice carrier, and zipped alongside the continuation — so every dep value is born at `'b`
through the one step `open`, with no separate slice reattach. The **active scope** opens at that same
brand: its carrier — the frame's own `SealedExtern<ScopeRefFamily>` for a `Yoked` slot, the node's
own `SealedExtern<ScopeRefFamily>` for a `YokedChild` — is zipped into the step `open` alongside the continuation, so the
dispatch decide reads `&Scope<'b>` from the one brand (and the consumer `dest` region is the opened
scope's own `region`, derived inside it) rather than re-anchoring a free `&Scope<'step>` up the
dispatcher stack. The frame-side reads fold onto `open` the same way: a frame's own child scope opens at
a `for<'b>` brand through [`CallFrame::with_scope`](../src/machine/core/arena.rs) — the `&mut self`
submit / classify paths reach it through `with_node_scope` / `with_current_node_scope`, copying out a
scalar (an id, a region) where they need no live scope — so no `&Scope` rides up a `&mut self` path. The
seed-side binds fold onto `open` the same way: the MATCH / TRY arm `it`-bind, the user-fn param-bind, and
the deferred-return-type elaboration each open the child scope at the brand through
[`CallFrame::with_scope`](../src/machine/core/arena.rs) and **relocate** their caller-`'a` value into the
opened scope's own region through the substrate (the erasing `alloc_object`, which forgets the caller
lifetime, for the `it` / param binds; the deferred return re-homing its elaborated `KType` into the
captured-scope region) before binding it — so the value lands at the brand and the seed fabricates no
free `&'a`. With every frame-side and
seed-side read on `open`, the access surface is `open` alone — the borrow-bounded `attach` accessor is
deleted.

## Storage choice belongs to the workload

**Generic.** The substrate is parametric over the witness `W`, and assumes nothing
about which storage backs a given carrier. A carrier may witness a freshly-allocated
region or borrow storage its creator already holds; the substrate routes both
through the same surface.

**In Koan.** The interpreter decides per node: a user-fn call installs a fresh
per-call region and witnesses its values with that frame's `Rc`; a sub-expression
node allocates into the *active* frame and witnesses with the caller's pin. A
tail-call chain reuses one node across a sequence of fresh frames. The substrate
imposes none of this — it is the workload's call, which is why "per-node memory" is
the carrier a node holds, not an arena the node owns.

The construction surface (`yoke` / `merge_pinned` / `with` / `map`, the witness-borrow
reattaches) is shipped, as is the relocation of the generic `Region<P>` allocator
beside its carrier in the `witnessed` module and the opaque [`Sealed`](../workgraph/src/witnessed.rs)
storage form (`seal` / `open`), with the node result slot rerouted onto it. Its **value reads** now
nest under the rank-2 `open`: two driver accessors copy out inside the brand — a value read
([`read_result_with`](../workgraph/src/scheduler.rs)) and a borrow-free error probe (`result_error`) — and the
three ride-up-stack dispatch sites resolve at the cart `'step` directly, so the transitional
self-witnessed `Sealed::read` is gone, and the scope channel — the frame-side reads and the seed-side
binds alike — fold onto `open` too, and the borrow-bounded `attach` is deleted. The witnessed
alloc surface has landed — region allocation hands back a foreign-reach-only
[`Witnessed`](../workgraph/src/witnessed.rs) with the active frame folded in at close, and the frame builder's
child scope is born externally-witnessed — and every object- and type-channel construction *terminal*
builds through it, a seal folding the already-witnessed carrier's reach rather than re-anchoring a
separately-built value. Allocation is reachable only through the branded
[`RegionBrand`](../src/machine/core/arena.rs) handle — a bare `&KoanRegion` exposes no `alloc_*`, so
"always witnessed" is compile-enforced for allocation. The construction-time scope re-anchor — a
per-call child's longer-lived lexical parent and root, content-shortened into the child's fresh region
under `Scope`'s invariance — is closed the same way: a same-region child stores its already-`'a` parent
by plain coercion, and the per-call frame child builds through the externally-witnessed construction
door [`build_frame_child_witnessed`](../src/machine/core/arena.rs), which brands the fresh region and
the foreign parent at one `for<'b>` and erases the child witness-less. No scope re-anchor survives
outside the witnessed substrate and the access surface is `open` alone, so "always witnessed" is a
closed type rule.
