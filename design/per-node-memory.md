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

## Construction: `yoke`, `merge`, `map`, and one wrapper per node

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
`Body::UserDefined` payloads), and a `KExpression`'s lifetime parameter is borne by exactly one
variant — `ExpressionPart::Spliced(Carried)`, the per-call resolved sub-result the scheduler folds
into a parent's parts. Raw, unevaluated AST is *splice-free*: it holds no `Spliced` part, so it binds
no live borrow and its `'a` is a phantom. `KExpression` is therefore a [layout-invariant carrier
family](#the-core-erase-store-witness-reattach) (the splice rides the layout-invariant `Carried`),
and an AST-embedding object yokes via `KoanRegion::alloc_witnessed_embedding`: it moves the owned
splice-free expression into the `yoke` closure, re-anchors its phantom lifetime onto the brand through
the safe-signature `reattach_with`, and allocs the object natively at the brand. Co-location is
enforced by the `for<'b>` brand exactly as for any leaf; the embedded AST contributes no region of its
own, and the sole residual obligation — that the embed is splice-free — is a `debug_assert`, not a
witness the type encodes.

What `yoke` cannot mint composes through `merge`: an aggregate folds its *element carriers* (deps
arriving witnessed from the lift); a closure folds the captured-scope operand minted from its frame
`Rc`. The object family's region-pure leaves and aggregates are built this way — a single-part
literal and a static aggregate cell `yoke` their owned data, and a list / dict / record folds its
dep carriers via `transfer_into` ([dispatch/literal.rs](../src/machine/execute/dispatch/literal.rs)
/ [single_poll.rs](../src/machine/execute/dispatch/single_poll.rs)) — co-location the `for<'b>`
brand enforces, never asserted. The carrier-self-building object constructions climb off `Witnessed::new`
the same way: the newtype / tagged-union [`constructors`](../src/machine/execute/dispatch/constructors.rs)
and [`catch`](../src/builtins/catch.rs) fold their dep carriers via `transfer_into` / `merge`, and FN
def [`finalize`](../src/builtins/fn_def/finalize.rs) `yoke`s its co-located `KObject::KFunction` onto a
carrier witnessed by the defining scope's frame. The value-embedding sites that take a *bare arg* —
[`attr`](../src/builtins/attr.rs)'s `Wrapped`, [`FROM`](../src/builtins/record_projection.rs)'s
`Record`, and the [literal.rs](../src/machine/execute/dispatch/literal.rs) Resolved arm's bound value —
climb off it the same way: each receives the value it embeds as a delivered
[`Sealed`](#storage-and-access-seal-open-transfer_into) carrier (`attr` / `FROM` through
[`BodyCtx::arg_carrier`](../src/machine/core/kfunction/action.rs), the Resolved arm through the binding
scope's own [`Scope::seal_value`](../src/machine/core/scope.rs)) and `merge`s it, so the projected object
names every region it reaches by construction. `Witnessed::new`, which pairs an *already-built* value
with an asserted witness, enforces no co-location — it asserts it in prose at the call site — so it
backs no construction terminal: the **type** family seals the same way — a region-pure or owned `KType`
through [`Scope::seal_value`](../src/machine/core/scope.rs), a region-referencing `KType::Module` through
[`Scope::seal_module`](../src/machine/core/scope.rs) (which folds its child scope's reach) — so every
multi-dep constructed value, object or type, is born co-located by the `yoke` brand. The region-pure
carrier is built by the purpose-built [`Witnessed::resident`](../src/witnessed.rs), which fixes the
witness to `W::default()` — the empty, pins-nothing set — so it cannot pair a value with a *wrong*
witness, only with the empty reach a region-pure value genuinely has; that emptiness is sound as a
within-step transient, the producing frame folded in at close ([`reseal_under`](../src/witnessed.rs))
before the carrier is stored. `Witnessed::new` keeps **no** blessed home: its surviving callers — the
bare-[`Done`](../src/machine/core/kfunction/action.rs)
[`finalize_terminal`](../src/machine/execute/finalize.rs) forward and the type / region operand
bundles — are transitional, asserting in prose the co-location the `yoke` / `merge` / `resident`
constructors enforce structurally.

**`merge` — fold many region-resident values into one.** Generic: a value built
from references into *two* regions cannot be bundled with one witness by `yoke`
alone. `merge` re-anchors two carriers at one shared brand, runs a projection that
binds one into the other, and re-seals under the **combined** witness — the union of
the two operands' regions, with `outer`-chain subsumption dropping a region another
already pins (`MergeWitness::merge`, which returns `None` only when the witness type
cannot represent the combination: a single-region witness whose operands are unrelated;
a region *set* always can). This is what keeps
witnessed-ness at the *boundary*: without it, an aggregate of independently-witnessed
elements would nest `Witnessed<…Witnessed<…>>` wrappers with the data and be
unstorable as a single node carrier. With it, the invariant holds:

> **One wrapper per node.** A node stores exactly one carrier, regardless of value
> complexity. `yoke` mints leaves into a region; `merge` folds region-resident
> values — same-region or cross-region — into one aggregate under the single witness
> that pins them all; the result seals as one unit. Wrapper count is O(1) per node,
> not O(data size).

In Koan, `merge`'s trigger is *referencing a pre-existing region-resident value* — the
foreign borrow a `yoke` closure would reject — and it is the **same-region** case almost
always: a list assembled in one call's arena, or a closure capturing its defining scope (a
`KFunction` is allocated *into that scope's region*, so the capture is co-located), where
subsumption trivially collapses the union to a single `Rc`. The genuinely
cross-region merges are *ancestry-related* — a scope or function in a per-call frame
referencing the run-global root (or a lexical-ancestor scope) — where the descendant frame
`Rc`'s `outer` chain already pins the ancestor region, so subsumption keeps the frame witness
and drops the ancestor's. The case `merge` *cannot* collapse — a value whose backing reaches an
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
passes and nothing branded escapes. (A borrow-bounded `attach<'w>(&'w self, &'w W) -> Live<'w>`,
re-anchoring capped at the witness borrow, is the accessor the **frame-side** scope reads route
through — the decide channel reads its scope from the step `open` instead; folding the frame-side reads
onto `open` too is tracked in [Open work](#open-work).) Bundling a witness the carrier does
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
`Rc` rides the *carrier*, which a node holds *outside* the region it witnesses; `merge` folds
every intermediate into that one carrier (the *one wrapper per node* invariant above), so no
region-resident value strong-owns its own frame — the value in-region holds only non-owning
pointers (a plain `&Scope`, a `Weak` `region_owner`). The per-call scope is the one value held
*inside* the frame, which is exactly why it stays externally-witnessed. A value that *captures*
the scope therefore has no bundled scope witness to `merge` against: it mints its merge operand
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
cross-region reference is the degenerate singleton (the destination alone). This is **not** a `merge`: the source is a dying
*descendant* of the destination (its ancestry pins the destination, not the reverse),
so no single dominating witness exists — the set is held whole and composed by union,
since splicing the source into the destination's `outer` chain to collapse it risks
re-forming the `src`↔`dst` cycle. This closes the one case `open` cannot: a value
whose source backing is dying but whose consumer outlives it. In Koan: the
consumer-pull lift across a dependency edge — `relocate_carried` copies the dep into the consumer
`dest` region at the step brand (the spine sharing its `Rc` payloads, a closure / future / module
riding its bare borrow), and `transfer_into` re-seals it under the set union of its reached sources
and `dest`. The lift delivers each dep **both** as a live bare `Carried` and as its producer slot's
own `Sealed` carrier (a `duplicate`): a finish that embeds or binds a value folds that carrier so the
reach is named on the carrier and never reconstructed. Every object construction does this — the
aggregate and region-pure inversions, the newtype / tagged-union constructors, and `catch` fold their
dep carriers via `transfer_into`; the bare-arg value-embedding sites (`attr`, `FROM`, the literal
Resolved arm) `merge` the [delivered carrier](#storage-and-access-seal-open-transfer_into) of the value
they project; and a `let` or user-fn arg bind folds the bound value's carrier into the scope reach-set
(below). The **type** channel rides the same construction: a type terminal seals via
[`Scope::seal_type`](../src/machine/core/scope.rs) — a region-pure or owned `KType` under the
producer's home frame, a region-referencing `KType::Module` via
[`Scope::seal_module`](../src/machine/core/scope.rs), which folds the child scope's home frame and its
sealed reach-set onto the carrier. A relocated module therefore names every region it reaches on its
own witness, read back at the consumer rather than reconstructed from the value. No finish reads a live
value out to rebuild its reach: the relocate-into-consumer seam is a plain
[`relocate_carried`](../src/machine/execute/lift.rs) structural copy, transient reach rides each dep's
carrier, and only a *bound* value deposits onto the scope reach-set (below).

A value *bound into a scope*, whose reach must outlive the binding node, deposits its reach on the
scope's own **reach-set**: a `FrameSet` held on the [`Scope`](../src/machine/core/scope.rs) that folds
each deposited value's carried reach (via `fold_reach`) and seals when the scope closes — `close` is the
seal point, fired at the owning node's finalize and routed per scope kind (a per-call frame at its body
slot's `Done` / tail-`Continue`; `MODULE` / `SIG` at the block finish; the run root at run end). The
bind sites deposit by folding the bound value's full delivered carrier across both channels: a
[`let`](../src/builtins/let_binding.rs) folds its bound value's carrier (an object RHS or a
resolved-type RHS alike), a user-fn arg bind folds each argument carrier — object and type — into the
*per-call* scope ([`exec::invoke`](../src/machine/execute/dispatch/exec.rs), the scope the parameters
bind on), and [`USING`](../src/builtins/using_scope.rs)'s transparent window folds the opened module's
carrier onto the call-site scope it borrows into. A multi-region value (a list of closures, a closure
over several closures, a module reaching a functor-result region) thus contributes *every* region it
reaches to the scope that outlives it.

`fold_reach` **omits ancestor regions** the scope already keeps alive: its own / a storage-`outer`
ancestor ([`FrameStorage::pins_region`]), *and* a **lexical** `outer`-chain ancestor. The lexical
omission is load-bearing under TCO: a per-call frame carries no storage `outer` link, so the storage
walk stops at its own region while a captured closure still pins its defining (lexical-ancestor) scope.
Folding such an ancestor into the reach-set — paired with a sibling bind of the call's result — would
close a `region → scope → set → frame` cycle and defeat the `Rc::get_mut` TCO frame-reuse gate; omitting
it realizes [`fold_foreign`](#construction-yoke-merge-map-and-one-wrapper-per-node)'s "omit ancestors"
intent while keeping a region-pure or ancestor-bound value depositing nothing.

With both channels' construction carried and binds folded, reach lives entirely on the node carrier
and, for bindings, on the per-scope sealed reach-set: a relocated value's reach is read off its own
carrier witness, never recovered from the value.

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
opened scope's own region through the substrate (a shortening `reattach_with` for the `it` / param binds;
the deferred return re-homing its elaborated `KType` into the captured-scope region) before binding it —
so the value lands at the brand and the seed fabricates no free `&'a`. With every frame-side and
seed-side read on `open`, the borrow-bounded `attach<'w>(&'w self, &'w W) -> Live<'w>` accessor is
**callerless** — kept only for the follow-up that collapses the access surface to `open` alone.

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

The construction surface (`yoke` / `merge` / `with` / `map`, the witness-borrow
reattaches) is shipped, as is the relocation of the generic `Region<P>` allocator
beside its carrier in the `witnessed` module and the opaque [`Sealed`](../src/witnessed.rs)
storage form (`seal` / `open`), with the node result slot rerouted onto it. Its **value reads** now
nest under the rank-2 `open`: two driver accessors copy out inside the brand — a value read
([`read_result_with`](../src/scheduler.rs)) and a borrow-free error probe (`result_error`) — and the
three ride-up-stack dispatch sites resolve at the cart `'step` directly, so the transitional
self-witnessed `Sealed::read` is gone, and the scope channel — the frame-side reads and the seed-side
binds alike — now folds onto `open` too, leaving the borrow-bounded `attach` callerless. The witnessed
alloc surface has since landed — region allocation hands back a foreign-reach-only
[`Witnessed`](../src/witnessed.rs) with the active frame folded in at close, and the frame builder's
child scope is born externally-witnessed — so what the per-node-memory roadmap project below still
tracks is migrating every remaining construction site onto it and collapsing the access surface to
`open` alone (deleting that callerless `attach`).

## Open work

The [per-node-memory roadmap project](../roadmap/per-node-memory/) tracks the remaining migrations.
The keystone run-loop restructure and its consuming `open`, the unified `FrameSet` set-witness, the
production witness impls, the `transfer_into` relocation verb, the per-value frame anchor's
removal (a stored value holds no owning `Rc` back to a region, so the allocation engine needs no
cycle gate), the per-scope sealed reach-set with its scope-close seal (the *Transfer* section
above), **both construction channels** — every object *and* type construction `yoke`s / `merge`s
or seals (`seal_value` / `seal_type` / `seal_module`) its reach, the bare-arg value-embedding sites
(`attr`, `FROM`, the literal Resolved arm) `merge` their delivered carrier, and `let` / user-fn arg /
`USING` binds fold their carriers into the reach-set — and the **value-read migration** — the
result-slot value reads nest under the rank-2 `Sealed::open` (a value copy-out and a borrow-free error
probe) and the three ride-up-stack dispatch sites resolve at the cart `'step`, so the transitional
self-witnessed `read` is deleted — have all landed (see
[Region lifetime erasure](memory-model.md#region-lifetime-erasure)).

What remains carries one goal to its end: an object allocated in a region is **always witnessed** —
the Witnessed/Sealed wrapper covers its whole lifetime, with no bare `&'a` reattach hole. The witnessed
alloc surface that opens this has landed: region allocation hands back a `Witnessed<T, FrameSet>` whose
set is the value's *foreign* reach, the active frame excluded and folded in only at close (the
scope-reach seal), so a region-resident value never strong-owns its own frame — the `region → object →
frame` cycle that would leak and defeat the `Rc::get_mut` TCO gate — and the frame builder's per-call
child scope is born externally-witnessed, proven end-to-end on a region-pure pilot. What is left is
breadth and enforcement: every remaining construction site migrates onto the surface, so the one
alloc-retype left lives inside `yoke`, and then the build-at-a-brand leaf is confined behind a branded
region handle, so a bare `&KoanRegion` cannot allocate at all and "always witnessed" is
compile-enforced. (The scope-pointer collapse has already landed, leaving the borrow-bounded `attach`
callerless for the last item to delete.) The two items carry this in sequence:

- [Witness value carriers at their construction site](../roadmap/per-node-memory/witness-at-construction.md)
  — every object- and type-channel construction folds reach via `yoke` / `merge` / `transfer_into` on
  the witnessed alloc; the bare `alloc_* -> &'a` callers and `reattach_with` are deleted.
- [One region handle, one access verb](../roadmap/per-node-memory/single-open-verb.md) — the build
  leaf moves behind a branded region handle (no bare-`&KoanRegion` alloc), the access surface
  collapses to `open`, and `attach` / `reattach_ref_with` / `recouple_scope` — the last retypes
  outside Witnessed/Sealed — are deleted.
