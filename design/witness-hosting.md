# Witness hosting: Koan's reach policy

Koan's values reach across regions — a closure captures its defining scope, a
module borrows a functor-result region, a bound argument stays resident in its
producer's frame. This doc owns the **policy** side of that: where a value is
allowed to escape its producer, how the copy-versus-pin choice is priced, what
Koan's eternal tier is, and how a move-in's residence is enforced.

The **representation** is the library's:
[workgraph/design/reach.md](../workgraph/design/reach.md) owns the reach
description / pin bundle split, the three carrier states and their transform verbs,
the holder rule, and the mint rules (self rule, subsumption, the eternal-rule
mechanism). [scheduler-library.md](scheduler-library.md) owns the library/embedder
boundary; [per-node-memory.md](per-node-memory.md) owns which construction verb each
Koan site takes and where a bind mints its reach.

Koan supplies three things to that machinery and nothing else: the frame-owner type
`Rc<FrameStorage>` with its `PinsRegion` subsumption hook, the `needs_no_pin` answer
that names its eternal tier, and the retention predicate at relocation sites. It has
no pin vocabulary — `PinBundle` is crate-private to the library — so the pinning
invariant is not a rule Koan is asked to honor but one it has no way to break.

## The carrier families

There is **one** value family per storable kind — objects via
[`CarriedFamily`](../src/machine/model/values/carried.rs), functions via
[`KFunctionFamily`](../src/machine/core/kfunction.rs). The witnessed library is
generic over `Reattachable` families, so a function is a family rather than a
carrier variant, and both ride the same three carrier states
([reach.md § The carrier states](../workgraph/design/reach.md#the-carrier-states)).

Koan's own type positions name `Delivered` — the one state the embedder can name —
where a value is in transit: a parked node slot, a dep terminal, a finish's result.
At rest a binding entry holds a `Sealed`; inside a step a read is an `Opened<'b>`,
typed at the step's `'step` lifetime by
[`StepCarried`](../src/machine/execute/step_carried.rs). Dispatch's picked overload
rides an adopted `Opened<'step, KFunctionFamily>` carried by `Resolved<'step>` across
argument evaluation and `reseal`ed into the `ReturnContract` that escapes into the
call chain.

## The eternal tier

The library's eternal rule keeps a member out of a region's union bundle when its
owner declares
[`PinsRegion::needs_no_pin`](../workgraph/src/witnessed/reach.rs). Koan's eternal
tier is the **run root**: `RegionHost::is_run_root` answers `true` for the storage
`interpret_with_writer_path` holds for the program's whole run.

The rule's correctness obligation is a drop-order one — answering `true` asserts the
storage stays live and fixed-address for at least as long as any region that could
retain it. Koan discharges it by construction: the run-root `Rc<FrameStorage>` is
created before the runtime and dropped after it, so every per-call region dies first.

The ring the rule cuts is concrete. A per-call region adopting a run-root-resident
module argument takes an owning `Rc` on the run-root host, while the run-root region
adopting that call's result takes an owning `Rc` on the per-call host. Neither edge
alone leaks; the pair is a cycle no `outer`-chain walk sees, because the per-call
frame's `outer` is `None` and the ring is expressed entirely through reach.

Acyclicity of Koan's region ownership graph therefore rests on three rules: the self
rule (no owned self-edge) and the eternal rule, both the library's, plus Koan's own
per-call frame rule — a frame's `outer` chain strong-owns only a **strictly older**
ancestor frame, a DAG and never a back-edge, so a dispatched frame chaining its
(possibly per-call) captured parent forms no cycle
([per-call-region/](per-call-region/README.md)). Regression coverage is
[`region_liveness.rs`](../src/builtins/fn_def/tests/region_liveness.rs), which runs a
program, drops the run, and asserts every `RegionHost` dropped with it.

## Escape: the single seam

A value escapes its producer frame in exactly one place: the **bind seam**, where a
consumer binds the delivered value into a scope. There is no second escape channel.

- A **declared return** (an FN's `-> :T`, a MATCH/TRY arm's contract) is checked and
  re-stamped **in place**, in the producer's own region, at the Done boundary. The
  check moves no bytes and re-homes nothing. The sealed return obligation is pure
  `Copy` data — the declared type is a run-region registry handle
  ([typing/type-registry.md](typing/type-registry.md)) and the error label is
  precomputed at seal — so the obligation references no region, holds no pin, and
  carries no relocation destination. Under TCO the obligation rides the tail chain
  keep-first and the check fires once, at the chain's end, exactly as
  [tail-call-optimization.md](tail-call-optimization.md) schedules it.
- An **undeclared return** ends the same way: the value stays in its producer frame;
  the scheduler's retention hold keeps that frame alive until every consumer pulls
  ([reach.md § Retention model](../workgraph/design/reach.md#retention-model)).
- At the bind seam the consumer prices **copy against pin**
  ([`adopt_disposition`](../src/machine/core/scope/reach.rs), the single home of the
  adoption rules, running the cost model of
  [value-substrates.md § Cost-driven copy](value-substrates.md#cost-driven-copy-the-optimization)):
  *copy* rebuilds the value in the destination region and lets the producer frame
  free at retention discharge; *pin* leaves the value in the producer's region and
  unions its pins into the destination region's union bundle, making that region the
  value's residence for the destination's life. Both are always legal; the choice is
  pure cost.

That choice reaches the pin arithmetic through the library's **retention
predicate** — `still_borrows(product, source)`, called on the product *after* the
fold has built it. A `false` verdict drops the source region from the composed
bundle; a `true` verdict keeps it. Koan may tune the predicate conservatively in
either direction: a conservative answer costs retention, never soundness.

Because pins are region-owned, a pinned residence ends when the binding's region
dies. The canonical example, spelled out:

```
FN count : n = MATCH (n) (0 -> 0) (_ -> count : n - 1)
```

Each tail hop retires its frame per retention. Bindings are bind-once and a tail
call is not known to re-enter the same function with a congruent slot set, so each
hop's `it` bind lands in a fresh scope: a loop-carried bind that priced to **pin**
would chain — iteration N+1's region bundle pins frame N, whose own region bundle
pins frame N−1, transitively. Every pin in that chain is droppable (each dies with
its scope's region), but a pinned loop holds O(N) retired regions until
[region evacuation](../roadmap/untyped_arena/region-evacuation.md) collapses the
chain at frame death. The bind seam therefore keeps its copy-bias for loop-carried
binds: the copy frees the producer at retention discharge, preserving the O(1)
region turnover TCO depends on.

Home = residence, by construction: a value is never moved out of its producer region
by any channel, so a `Delivered`'s home member, the producer's retention hold, and
the value's residence region are one and the same region.

## Scope and bindings above the substrate

The Koan layers compose the substrate; neither the scope nor its binding entries
hold any witness state — the pins live one level down, in the library's region.

- **Binding entries are `Copy` and `Drop`-free.** Both binding tables store one
  `Sealed`-shaped carrier beside a `BindingIndex` —
  `data: name → (BindingIndex, Sealed<CarriedFamily>)` and
  `functions: key → Vec<(BindingIndex, Sealed<KFunctionFamily>)>` — so a value is
  never separated from the reach that proves it. An entry owns no pins; its liveness
  is the region's union bundle.
- **The scope holds no pin state at all.** Binding a value mints its description
  into the scope's region and unions its pins into that **region's** own deduped
  bundle, applying the self rule before insertion. The region carries one pin per
  distinct foreign region, not one bundle per entry. The mint and the value's
  construction are **one fused door**
  ([`Scope::adopt_for_binding`](../src/machine/core/scope/reach.rs), `seal_pure_value`,
  `seal_module` and siblings), so a scope entry cannot state a reach the value's
  borrows don't back, and the union is written by the library rather than by the
  door's caller. The door's product is a resting `Sealed`; the table write itself is
  a separate, run-loop-owned step
  ([memory-model.md § Binding writes ride the step outcome](memory-model.md#binding-writes-ride-the-step-outcome)).
- **Reads stay refcount-free.** A binding read opens the entry's `Sealed` under the
  region's own coverage (`open_at`) and hands out an `Opened<'b>` enveloped by the
  region's union bundle; pins are adopted only when the value genuinely escapes to a
  new holder — a new region.
- **Module reach** is the union over the child scope's entries, composed once at scope
  close by the module store fold
  ([`Scope::store_module_object`](../src/machine/core/scope/reach.rs)), which merges
  the resident module reference into the storing scope's region; the parent
  region's union bundle owns the resulting pins. A relocated module therefore names
  every region it reaches on its own witness, read back at the consumer rather than
  reconstructed by walking the built value.

## Residence enforcement

A composite value's **residence** — every region its borrows reach is covered by the
destination — is discharged **at construction, by the fold brand**, not by a runtime
walk. A relocation or bind builds the value inside a `for<'b>` fold closure
([`FoldingBrand::alloc_object_folded`](../src/machine/core/arena.rs)), where the only
inhabitants of `KObject<'b>` are the fold's declared operand views, the brand's own
allocations, and owned data — all named by the witness the enclosing combinator
composes. An ambient-lifetime capture is a compile error at the closure signature, so
the store is sound with no per-value audit. Copy and pin both take this door: a
copy's fold structurally rebuilds into the destination; a pin's fold pointer-copies
the source under the composed `Kept` witness that names its producer host. This is
the residence analogue of the library's description/pins compile-safety line — a
move-in that cannot name its reach does not typecheck.

**No residence walk survives.** Nothing anywhere confirms residence by visiting a
composite value's contents. The three shapes a fold brand does not cover reach their
destination through a door whose *signature* is the enforcement instead:

- **A region-free leaf** — `Number` / `Bool` / `Null` — is spelled
  [`Scalar`](../src/machine/model/values/kobject.rs), a type with no lifetime at all,
  so a value borrowing any region cannot be written as one and `alloc_scalar` has
  nothing to check. `alloc_string` is its sibling for the leaf whose bytes are
  region-hosted: re-homing them into the destination *is* the store.
- **Raw AST** takes `RegionBrand::alloc_expression`, which admits a
  [`ProgramExpression`](../src/machine/model/ast/program.rs) and nothing else — the marker
  minted only through a [`ProgramBrand`](../src/machine/core/arena/frame.rs) door. A
  `KObject::KExpression` needs no *coverage* claim of its own either: the marker on its
  payload is the proof that the node's parts run is eternal-tier program storage, and the
  one part kind that could name a producer region lives on the scheduler's own node type,
  which no value can hold
  ([value-substrates.md § Value-channel AST](value-substrates.md#value-channel-ast-the-program-storage-marker)).
- **A fresh `KFunction` wrapper** takes
  [`Scope::store_function_object`](../src/machine/core/scope/reach.rs), a merge modelled
  on the module store fold: the composition mints the callable's home region into the
  product's reach, which is the borrows-home fact the wrapper carries, and the self rule
  strips it from the retained bundle. The claim is exact — a `KFunction`'s only region
  borrow is its captured scope, whose own sealed reach set transitively covers everything
  its bindings reach.

A carrier-less argument routes those shapes through
[`Scope::place_pure_value`](../src/machine/core/scope/reach.rs). Every other shape borrows
a region no such door can name, so it reaches its destination as a delivery envelope
instead, and arriving at the pure door is a construction bug reported as a diagnostic —
not a residence verdict a caller could turn into an admission.

**No runtime residence check survives either.** A `KFunction`, `Scope`, or `Module` borrows a
single region (its captured / parent / child scope), and each is *born at its destination*: the
value is constructed and stored in one act inside a `for<'b>` brand over the destination region
([`RegionHandle::alloc_resident_born`](../workgraph/src/witnessed/region.rs) and its
crossing-operand sibling `alloc_resident_born_with`), so the region pointer it carries is the
destination's by construction. The koan doors —
[`Scope::alloc_child_under`](../src/machine/core/scope.rs) and its siblings,
[`KFunction::alloc_captured`](../src/machine/core/kfunction.rs),
[`Module::alloc_at_child_scope`](../src/machine/model/values/module.rs) — each derive the
destination handle from the value's own anchoring scope rather than taking a brand alongside it,
so pairing a value with a foreign region is not stateable at a call site. The value-returning
constructors are private, so none of the three can exist outside the act that stores it. A
`Module` re-tagging a *foreign* child scope (a transparent ascription view) takes the fold brand
instead, inside the fold that merges that scope in
([`Scope::store_transparent_view`](../src/machine/core/scope/reach.rs)).

An operand the born closure has to embed but cannot derive from the brand — the per-call frame
child's lexical parent, which lives in another region — crosses through the witnessed channel as a
`SealedExtern` re-anchored to the same `'b`, under a [`Witness`](../workgraph/design/witnessed-memory.md)
pin borrowed for the *destination region's* own lifetime. That is what keeps the door a
lifetime-shortening rather than a lengthening: the pin's contract covers the stored reference's
whole life, not merely the call. Co-location of that pin with the operand stays a caller
obligation, the same one `SealedExtern::open` already carries; the door narrows its duration
rather than adding an obligation class.

Where a seam has to *ask* where a composite lives — the copy-versus-pin decision's
home-crossing test — it reads the answer off the value:
[`ContainerSubstrate::homed_in`](../src/machine/model/values/container_substrate.rs)
compares the substrate's own stored description's host region by pointer. A region keeps no
address table at all, so there is nothing else to consult; and nothing else is needed,
because the door that placed the substrate is what made the stored host true.
