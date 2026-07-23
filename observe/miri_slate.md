# Miri audit slate

<!-- slate-fingerprint
src/machine/core/arena/residence.rs: 4
-->

The canonical list of tests Miri's tree-borrows mode signs off on for koan's
memory safety. Each test is a minimal-shape mirror of an unsafe site in the
runtime; the slate passes when Miri reports zero process-exit leaks and zero UB
across the whole list.

Command of record and triage workflow live in
[.claude/skills/miri/SKILL.md](../.claude/skills/miri/SKILL.md). Memory-model
invariants the slate verifies live in
[design/memory-model.md](../design/memory-model.md).

## Stale-group whitelist

Slate groups whose anchor file no longer carries `unsafe` because the test pins
a safe-code invariant (e.g. a `RefCell` discipline that tree borrows can still
violate). `slate-audit` skips the stale-group check for these paths only; new-
unsafe and fingerprint-drift checks still fire.

**Whitelisting is not automatic when an `unsafe` is removed or relocated.** A slate
test earns its place — and a whitelist entry — only if it can catch a memory error
*no other slate test catches*. When an `unsafe` site is deleted (or its backing op
moves to another file, e.g. a layout-invariance `unsafe impl` folded into the
`reattachable!` macro whose home is `witnessed.rs`), ask of each test under the now
anchor-less group: does it still pin a distinct UB shape? If yes — keep it and
whitelist the anchor here, citing the shape and where the real `unsafe` now lives. If
no — the test is redundant; **delete it** rather than whitelist. Do not whitelist a
group just to silence the stale-anchor check.

<!-- slate-audit-whitelist:start -->
- `src/machine/core/arena.rs` — arena.rs split into `arena/{frame,step_allocator,residence}`
  child modules. Its remaining groups (CallFrame lifetime erasure, reference-only carrier
  retention, multi-region union, witness-set hosting, `alloc_carried_with`, MATCH-Tagged / TRY-WITH
  TCO, per-call frame re-anchor, NodeStore reinstall) pin safe-code frame / carrier / region
  drop-order and reattach disciplines whose backing `unsafe` is the `Region::alloc` retype in
  `witnessed.rs`; the `unsafe impl AuditedStored` audits that gate those stores moved to
  [`arena/residence.rs`](../src/machine/core/arena/residence.rs). arena.rs itself carries no
  `unsafe` of its own.
- `src/machine/core/scope.rs` — `Scope::add` re-entry pins the queue-and-drain
  discipline that keeps `Scope`'s `RefCell<…>` invariant intact when a binding
  is added while a `data` borrow is live.
- `src/machine/core/kfunction.rs` — `KFunction::captured_scope` is a bare field read of the
  stored `&'a Scope<'a>` (re-anchored with the holder by the `Region::alloc` retype), so
  kfunction.rs carries no `unsafe` of its own. The group pins the captured-scope-survives-
  closure-escape and delivered-carrier reach-fold shapes.
- `src/machine/model/values/module.rs` — the `Module` groups pin a safe `RefCell`
  discipline (interior mutation under a live `&'a Module`) and the MODULE-body
  Combine continuation; the captured-scope re-anchor they reference is the stored `&'a Scope<'a>`
  re-anchored with the `Module` carrier by the `Region::alloc` retype in `witnessed.rs`, so module.rs
  carries no `unsafe` of its own.
- `src/machine/execute/outcome.rs` — the `ContinuationFamily` group's test
  (`erased_continuation_open_roundtrip`) pins the **fat-pointer** (`Box<dyn>`)
  erase → open → invoke round-trip — a layout shape no thin-carrier test covers.
  The real `unsafe` is the `Erased::reattach` inside `SealedExtern::open` in
  `witnessed.rs`; the family's `unsafe impl` is `reattachable!`-generated, so outcome.rs
  carries none.
- `workgraph/src/scheduler/node_store.rs` — the slot-read group pins `read_result_with`'s
  `open_with` under the retained frame owner (a safe pinned open; the `unsafe` lives in
  `witnessed.rs`) via an end-to-end tail-chain return-contract-coarsening shape no
  minimal test reproduces. The file's only former `unsafe` was the test-family markers,
  now `reattachable!`-generated.
- `src/machine/execute/nodes.rs` — `node_scope_yoked_child_erase_open_roundtrip`
  pins the `NodeScope::YokedChild` erase → open round-trip plus a sibling-pointer
  region mutation — an `erase_to_static` → `SealedExtern::open` shape through the scope carrier
  that no value-family test reproduces. The open routes the fully-safe
  `SealedExtern::open` on a stored `&'static Scope`, whose only `unsafe` (the
  shared `retype`) lives in `witnessed.rs`, so nodes.rs carries none of its own.
- `src/machine/core/scope_ptr.rs` — every holder stores its captured / defining / parent scope as a
  plain `&'a Scope<'a>`, re-anchored **with the holder as a whole** by the `Region::alloc` retype in
  `witnessed.rs` (the construction-time reference is built at `'a` by plain coercion for a same-region
  child, or at the construction door's brand for a per-call frame child), so scope_ptr.rs carries no
  `unsafe` of its own. The group pins the stored scope-pointer re-anchor shape.
- `src/machine/execute/dispatch/ctx.rs` — the `with_node_scope` read boundary is the
  sole production open of a `YokedChild` carrier; it passes the executing slot's
  cart `Rc` as the witness to `SealedExtern::open`, a **safe** call, so ctx.rs carries no
  `unsafe`. The group pins that boundary end-to-end (every scheduler-driving slate test); the
  `unsafe` it routes lives in `witnessed.rs`.
- `src/machine/execute/lift.rs` — `copy_carried` structurally copies at the brand a step open
  supplies (safe allocs; the former value-relocation `unsafe` was deleted with the per-value anchor).
  The group pins the escaping-value **retention** discipline — a surviving closure / module borrow
  kept alive by the consumer frame's `retained` `FrameSet` — which tree borrows catches if it
  regresses.
- `src/machine/core/carrier_witness.rs` — the reference-only collapse moved every
  `unsafe impl` off this file onto the library `Carrier<F>` in
  `workgraph/src/witnessed/carrier.rs` (a separate crate the koan-scoped fingerprint doesn't track);
  `carrier_witness.rs` is now the `CarrierWitness` / `DeliveredCarried` type aliases. The group's
  tests still pin real memory-safety shapes — the reference-only carrier under its retention hold
  and the `compose` mint — just via that library type, not this file's own code.
- `src/machine/execute/run_loop.rs` — `run_step`'s dep-union `pin` is built entirely through safe
  envelope/`RegionSet` verbs (`Delivered::liveness_frameset`, `FrameSet::union`/`singleton`); the
  file carries no `unsafe` of its own. The group pins the retention redundancy claim — a dep's
  producer frame is held by its `DepTerminal`'s duplicated delivery envelope across the step open,
  not by `run_step`'s `pin` alone — the real `unsafe` it exercises is the shared `retype` in
  `witnessed.rs`, routed through the `Sealed`/`SealedExtern` opens `run_step` and the dep reads
  perform.
<!-- slate-audit-whitelist:end -->

## The slate

44 tests, grouped by the unsafe site each pins down. Names below are the exact
test identifiers; pass them after `--` in the Miri command. A further 21 tests
covering the witnessed substrate live in the `workgraph` crate's own slate
([workgraph/observe/miri_slate.md](../workgraph/observe/miri_slate.md)).

**`CallFrame` lifetime erasure** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — the
child-scope `Option<SealedExtern<ScopeRefFamily>>` opened at a `for<'b>` brand via `CallFrame::with_scope`
(`SealedExtern::open`, the frame's own storage `Rc` as the pin) plus the `Rc<CallFrame>` chain that
keeps per-call regions pinned across re-borrow. One test pins the open surviving a sibling alloc; one
pins the `Rc<CallFrame>` chain keeping an outer region alive after its local handle drops; a third pins
the **seed-side re-anchor** — a caller-lifetime value relocated into the opened scope's own region
through the substrate (the erasing `alloc_object`, which forgets the caller lifetime and re-homes the
value at the region) and bound, the shape the MATCH / TRY `it`-bind and the user-fn param-bind take. `CallFrame::adopting` (the scheduler-owned run
frame) carries the same `&Scope<'_>` erasure as `new`, over the run scope it adopts rather than a
freshly-minted child; it is built on the first run-lifetime submission, so every scheduler-driving slate
test below (`recursive_tagged_match_no_uaf`, `lift_park_minimal_program_for_miri`, …) exercises it
end-to-end — the run scope outlives the frame, so no separate minimal test.

- `call_frame_scope_survives_subsequent_alloc`
- `call_frame_chained_outer_frame_walkable`
- `with_scope_relocates_seed_value_into_brand`

**Record substrate door — construction, O(1) ownership, fold-shared retype** ([src/machine/core/arena.rs](../src/machine/core/arena.rs))
— `FoldingBrand::alloc_record_folded` (the sole `RecordSubstrate` mint, routed through by
`KObject::record_of_held`) stores the substrate into its own brand's region exactly like
`alloc_object_folded`, so it carries no `unsafe` of its own beyond the `reattachable!`-generated
layout-invariance audit in `witnessed.rs`. One test pins the store landing in the brand's own
region, a hit for both the bare `KoanRegionExt::owns_record` address query and
`Residence::owns_record`'s dest-only case. A second pins `alloc_carried_with`'s fold-brand
construction one level up for a `Record` specifically — mirroring
`object_field_reach_fold_survives_producer_frame_free`'s `KFunction` shape, but for
`KObject::record_with_type` (FROM's own construction): the narrowed record shares the exact same
substrate pointer (never copies) built from a delivered `record` operand's view in a *different*
frame, and the fold's reach union is what keeps the producer's region alive once every producer
handle drops — the shape `record_projection::body`'s `alloc_carried_with(&[lhs], …)` call takes in
production.

- `alloc_record_folded_stores_and_owns_a_record_substrate`
- `record_retype_shares_substrate_across_producer_frame_free`

**`Region` alloc engine under live borrows** ([workgraph/src/witnessed/region.rs](../workgraph/src/witnessed/region.rs)) — the
single `store` path erases the value to `'static` (the move-through-union `erase_store`), writes it to
the sub-arena, and records its address into the `membership` `RefCell` via `borrow_mut`; two surfaces
re-anchor it, both pinned here while a prior `&` from the same region is shared-borrowed. The bare-`&'a`
`alloc_resident` re-anchors to `'a` through the tight in-module `retype` leaf — content == borrow ==
`'a`, capped by `&'a self`, region.rs's one `unsafe` (`region_alloc_while_prior_ref_live`). The
brand-confined `alloc` hands the
freshly-stored value to a `for<'b>` closure through `with_branded_ref`, letting only the erased carrier
escape — the closure-surface twin pins the store → record → brand-read → sibling-alloc composition
(`alloc_engine_brand_coexists_with_sibling_alloc`). Both over the `KoanRegion`
(= `Region<KoanStorageProfile>`) the engine routes.

- `region_alloc_while_prior_ref_live`
- `alloc_engine_brand_coexists_with_sibling_alloc`

**Reference-only carrier — retention-held read across shell drop** ([src/machine/core/arena.rs](../src/machine/core/arena.rs))
— a region-pure object allocated through the brand-confined `alloc_object_witnessed` is born under the
empty reach, so its carrier pins **nothing**. Sound because reads never go bare: the active frame pins
the region during the producing step, and at finalize the scheduler seeds a retention hold on the
producer's storage that rides the delivery envelope (`Delivered`) to every consumer. The test pins that
hold across the producer shell's drop — seal the carrier as-is into its envelope (host = the storage
`Rc`, the hold's stand-in), then drop the producer shell outright (a `FreshTail` tail hop mints a fresh
cart and drops the retiring one rather than resetting it in place); the retained storage keeps the
region (where the value lives) alive, so opening the envelope after the drop reads a live pointee.
Without the hold the empty carrier would pin nothing and the drop would free the region under the
stored carrier. The only `unsafe` it routes is the shared `retype` in `witnessed.rs` (through the
envelope's pinned open).

- `reference_only_carrier_survives_producer_shell_drop_under_retention_hold`

**Multi-region union — envelope folds over independently-dying regions** ([src/machine/core/arena.rs](../src/machine/core/arena.rs))
— these tests hand-build genuinely multi-region carriers — a value reaching several
*independently-dying* per-call regions — through the delivery verbs only (`Delivered::transfer_into`
folds each element onto a `yoke_branded` accumulator, minting its regions into the destination arena;
`map_pinned` under the destination's retained storage builds the final value — never a hand-assembled
witness), free every producing frame, then read a reached closure's captured scope back: a
use-after-free under tree borrows the instant the minted set under-counts (a single frame witnessing
the whole aggregate frees the others' regions). The three shapes split the fold's two liveness
channels across the design's multi-region cases. The **list** elements ride the LET-bind →
entry-re-read pipeline (closure whole in its own home region, envelope host = the *reader* frame
whose arena holds the minted entry reach), so the closure regions arrive as element **reach** the
fold must union — host materialization alone covers only the readers. The **record** fields and the
closure-capturing-closures **reach tree** travel producer-hosted (host = the closure's own frame,
carrier empty), so their regions arrive as **residence** the `Residence::Kept` fold must
materialize; the reach-tree shape further folds its outer closure at a host that *is* the
destination frame, minting the aggregate's `borrows_into_home` bit set where the list's and
record's stay unset. The only `unsafe` routed is the shared `retype` in `witnessed.rs` (through
`yoke_branded` / `transfer_into` / `map_pinned`).

- `multi_region_list_of_closures_survives_frame_free`
- `multi_region_closure_capturing_closures_survives_frame_free`
- `multi_region_record_of_closures_survives_frame_free`

**Envelope transfer — cross-region residence mint and pass-through duplication** ([src/machine/core/arena/residence.rs](../src/machine/core/arena/residence.rs))
— the delivery-envelope relocation seam
([workgraph/src/witnessed/delivered.rs](../workgraph/src/witnessed/delivered.rs)): a
`Residence::Kept` `transfer_into` of a foreign region-resident element mints the envelope's host into
the destination's arena as an ordinary reach *member* rather than dropping it (the value keeps living
in the producer's region) — the direct unit-level twin of the `multi_region_*` shapes above, minus the
aggregate-fold machinery. A use-after-free under tree borrows the instant the transfer drops the
foreign host instead of materializing it. The duplication twin pins the walking half: duplicating an
envelope for dep delivery bit-copies the reference-only carrier and clones exactly one `Rc` (the
retained host) — the reach set itself rides by reference, never re-minted, so a regression shows as
per-member refcount traffic or a leak. The `unsafe` routed is the shared `retype` in
`witnessed.rs` plus `Carrier`'s own `with_reach` pinned re-anchor; the anchor file
[`arena/residence.rs`](../src/machine/core/arena/residence.rs) additionally houses the four
`unsafe impl AuditedStored` family audits — the soundness markers gating every checked
cross-region store this seam and its siblings exercise.

- `envelope_transfer_folds_an_independent_foreign_value`
- `pass_through_duplicate_keeps_reach_pointer_and_mints_nothing`

**Record substrate — checked-tier O(1) membership** ([src/machine/core/arena/residence.rs](../src/machine/core/arena/residence.rs))
— `resident_in_visiting`'s `Record` arm (`residence.owns_record(substrate)` in
[kobject.rs](../src/machine/model/values/kobject.rs)) is reached only when a record rides inside a
still-`Rc` container (`List`/`Dict`/`Tagged`/`Wrapped`) crossing the checked tier
(`Scope::alloc_object_delivered`) — a bare top-level record never routes this walk (born resident
by construction through the fold door). This test drives a `List` embedding a `Record` through the
checked tier twice: once with evidence naming the record's home region (must pass, reading the
address table, never the record's fields — the O(1) membership check), once without (must reject),
proving the arm is a genuine membership check rather than an always-true stand-in. The `unsafe`
routed is the same four `unsafe impl AuditedStored` family audits the group above exercises.

- `record_nested_in_list_crosses_checked_tier_via_owns_record_membership`

**Witness-set hosting — mint self-cycle / teardown** ([src/machine/core/arena.rs](../src/machine/core/arena.rs))
— `RegionSet::mint` (mechanism in
[workgraph/src/witnessed/region_set.rs](../workgraph/src/witnessed/region_set.rs), exercised here
over Koan's own `FrameStorage`) stores a frozen `FrameSet` into a destination arena through the
same `alloc_resident` engine the `Region alloc engine` group already pins — its own body has no
`unsafe` — but it introduces the one **cycle shape storage-side reasoning can't rule out**: a set
hosted in region A holding `Rc<A>` would be a strong self-cycle A never drops. Home-omission is
the discipline that forbids it (design/witness-hosting.md § The shape); the drop-order test is the
leak-audit gate that catches a home-omission regression — under plain `cargo test` the refcount
assertions alone would only ever *fail loud*, but it is the Miri run over this exact test that
signs off "0 leaks" for this shape specifically.

- `mint_teardown_releases_members`

**`CarrierWitness` = the reference-only `Carrier<FrameStorage>`** ([src/machine/core/carrier_witness.rs](../src/machine/core/carrier_witness.rs),
mechanism in [workgraph/src/witnessed/carrier.rs](../workgraph/src/witnessed/carrier.rs)) — the
library carrier is a `Copy` `{ borrows_host, reach }` description that is deliberately **not** a
`Witness`: a bare `Sealed::open` under it does not compile, so every read names its coverage — an
explicit pin (`open_with` / the `*_pinned` verbs) or the delivery envelope's retained host. Its one
`unsafe impl` (`ComposeWitness<B>`) asserts the pure mint: `compose` mints `left`'s exact reach into
`right`'s (the destination's) arena via `RegionSet::mint` — never a hand-assembled union — and
materializes no residence host (`compose` holds none); hosts fold only through the envelope verbs
(`Delivered::mint_reach` / `transfer_into`), which alone carry the host and the `Residence` mode. The
multi-region-union tests and the envelope-transfer tests above route entirely through this type. No
`unsafe` beyond the impl's contract and the pinned `with_reach` re-anchor: the erase/reattach
otherwise routes the shared `retype` in `witnessed.rs`.

**`alloc_carried_with` finish-surface reach fold** ([src/machine/core/arena.rs](../src/machine/core/arena.rs))
— `KoanStepContextExt::alloc_carried_with` routes a finish's result through the
library combinator `StepContext::alloc_with`, folding each listed dep's sealed reach into the
result's witness by construction before the caller's `build` closure ever holds a dep-derived
borrow. This test seals a closure resident in a producer frame's region — its captured-scope
borrow is the pointee at stake (the stand-in for a dep terminal's `t.value`/`t.carrier`) — into a
*different* consumer frame's own carrier via `alloc_carried_with`, the dep's view riding in as a
`Held` cell rebuilt at the fold brand; it then drops the dep envelope and every producer-frame
handle and reads the captured scope back — a use-after-free under tree borrows if the fold is
skipped. The only `unsafe` routed is the shared `retype` in `witnessed.rs` (through `alloc_with`'s
`yoke`/`merge`).

- `object_field_reach_fold_survives_producer_frame_free`

**`KFunction` captured-scope re-borrow** ([src/machine/core/kfunction.rs](../src/machine/core/kfunction.rs)) — every
closure invocation reads `KFunction::captured_scope`, now a bare field read of the stored
`&'a Scope<'a>` (re-anchored with the holder when it is read out of its region). The
escaped-closure test pins that the pointee outlives the `KFunction` even when the closure is
invoked after its defining frame has returned.
The reading-the-captured-value tests further pin the **delivered-carrier reach fold**
that keeps that defining region alive once the object channel is off the relocate seam: a
`let`-bound closure folds its carrier into the binding scope's reach-set, a user-fn
closure argument folds into the per-call scope, and a `let`-bound list contributes
*every* region a multi-region value reaches (the case the single-frame seam fold
under-recorded). Each reads a captured *outer* value after its producing frame retires, so
a lost region dangles under tree borrows.

- `fast_lane_closure_escapes_outer_call_and_remains_invocable`
- `captured_per_call_value_survives_let_bind_and_call`
- `closure_argument_stays_live_through_user_fn_call`
- `let_bound_list_reaching_two_call_regions_keeps_both_live`

**`Scope::add` re-entry** ([src/machine/core/scope.rs](../src/machine/core/scope.rs)) — adding a binding while
a `data` borrow is live queues onto a pending list and drains on borrow drop,
so the conditional-defer path doesn't violate the `RefCell` invariant. (Safe
code by typestate; pinned in the slate because tree borrows catches the
violation if the queue/drain discipline regresses.)

- `add_during_active_data_borrow_queues_and_drains`

**`Scope::adopt_sealed` reach-fold reattach** ([src/machine/core/scope.rs](../src/machine/core/scope.rs))
— the consumption verb re-anchors a foreign producer's sealed carrier at the consumer scope's own
lifetime (`Erased::reattach` to `'a`), copy-free, pinned by the reach `Scope::host_reach_of` mints
into the consumer's own arena **before** the reattach. This test seals a value witnessed by a
producer frame, adopts it into a consumer scope in a *different* frame, drops every direct producer
handle, then reads the adopted value — so the minted reach is the sole pin on the region the
re-anchored borrow reads, and tree borrows catches a use-after-free if the mint-then-reanchor order
or the pin regresses.

- `adopt_sealed_reach_fold_pins_the_producer_region_after_drop`

**`Scope::adopt_sealed` delivered re-home across retention** ([src/machine/core/scope.rs](../src/machine/core/scope.rs)) —
adoption consumes a *delivered* cell: the mint (run first in `adopt_sealed`, at `Residence::Kept` —
the envelope's host always materializes) pins the producer's residence and the value's foreign reach
into the consumer's arena before the copy-free `Erased::reattach` fabricates the consumer-lifetime
borrow. This test finalizes an object at the Done boundary (mirroring production), rides the
retention hold across the producer shell's drop, adopts into an independent consumer scope, then
drops the hold and every other source handle before reading — the consumer's minted set is the sole
owner at the read, pinning the hold-to-mint handoff. Tree borrows catches a use-after-free if the
mint-before-reattach order, the host materialization, or the pin regresses.

- `adopt_sealed_object_rides_retention_across_producer_shell_drop`

**Dep envelopes held across a step's own open** ([src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs))
— `run_step`'s consumer-step `pin` is a plain `FrameSet` folded from each dep envelope's
[`liveness_frameset`](../workgraph/src/witnessed/delivered.rs) (retained host ∪ reach). The
redundancy claim this is sound on: `dep_sources`' own `DepTerminal`s each hold the dep's *duplicated*
delivery envelope (owning the retention hold's `Rc` directly) across the whole step brand, so a
producer frame's liveness never rests on `pin` alone. This end-to-end test drives 100 real scheduler
steps each producing a region-pure scalar result, aggregates all 100 into one list literal — a single
consumer step opening 100 delivered deps at once, each folded at `Residence::Copied` (the aggregate
deep-clones its cells, so no producer materializes into the aggregate's reach) — and confirms every
producer arena is gone while the aggregate still reads correctly: a use-after-free under tree borrows
the moment the redundancy claim is wrong, and a lifetime leak (the census reads live frames) the
moment a `Copied` fold re-pins a producer it copied out of. The only `unsafe` routed is the shared
`retype` in `witnessed.rs`.

- `aggregate_of_call_results_releases_every_producer_frame`

**`Scope::child_module_reach` seal-time union** ([src/machine/core/scope.rs](../src/machine/core/scope.rs))
— a module's stored reach is minted once at seal time as the union of its child scope's own region
plus every one of the child's **binding-entry** hosted reaches (not just the child's own region), via
`Bindings::entry_reaches`. This test binds a member into a child scope whose stored reach names a
region foreign to both the child and the parent, then mints the parent's union and drops every other
handle on both regions — tree borrows catches a use-after-free if the union drops a member's reach or
the mint's home-omission fires on the wrong side.

- `child_module_reach_unions_member_entry_reaches_across_regions`

**`USING … SCOPE` transparent-window aliasing** ([src/machine/core/scope.rs](../src/machine/core/scope.rs)) — a
`ScopeBindings::Borrowed` window reads another scope's `RefCell` maps through a
borrowed reference, and the block (run in a transparent scope allocated in the
call-site region) can define a closure that escapes carrying that window. Pins
that an escaping closure reading a surfaced member of a *temporary* functor-result
module — the harder case, relying on the call-site-region `Rc` rooting — does not
dangle into the freed module/USING region. (Safe code by construction; pinned
because tree borrows catches a regression in the aliasing or rooting discipline.)
A second shape pins the transitive-root exception on `Scope::resident_witness`: a value read through
the window carries a reference-only carrier whose reach set lives in the **module's own arena**,
sound only because `USING`'s own overlay fold mints the opened module's carrier into the call-site
arena before any such read — so the call-site frame (held by its retention hold) roots the module's
arena one hop removed, and through it the read entry's reach set.

- `using_temporary_functor_result_is_sound`
- `using_window_value_read_reach_survives_under_module_root`

**MATCH on `Tagged` recursion** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — MATCH
builds its per-call frame and seeds its `it` bind through `CallFrame::with_scope`: the matched value,
deep-cloned at the caller lifetime, is relocated into the opened child scope's own region through the
substrate (the erasing `alloc_object`, which forgets the caller lifetime) and bound; the `FrameStorage` ancestor chain keeps the
call-site region alive across TCO replace when a user-fn recurses through a `Tagged` parameter via
MATCH.

- `recursive_tagged_match_no_uaf`

**Tail-hop argument adoption ordering (Lemma 2)** ([src/machine/core/scope.rs](../src/machine/core/scope.rs)) — a
tail call's loop-carried argument is delivered as its envelope (host = the retiring incarnation's
frame) and adopted by copy (`Scope::adopt_sealed_copied`, the `Residence::Copied` mint): the copy's
interior borrows are re-pinned by the adopted-reach mint before the copy's `&'a` is fabricated, while
a residence-only host (`borrows_host` unset) is left unminted and released with the retiring hold —
so the retiring region frees strictly after the adoption copy reads it. The test rebuilds an aggregate from the previous hop's own
carried value at every hop, so the spliced carrier genuinely pins the retiring region across the hop;
tree borrows catches a use-after-free if the free ever reorders before the adoption read. A second
test drives the record-embedding twin of this same adoption path — `Scope::copy_delivered_record`
([scope/reach.rs](../src/machine/core/scope/reach.rs))'s `embeds_record` branch, taken instead of the
plain deep-clone arm whenever the loop-carried argument is (or embeds) a `Record`: each hop threads a
fresh `{acc = …}` record argument through `THREAD`'s `it`-bind, which the seam copy verb totally
rebuilds into the callee's per-call region and — because the record borrows nothing — releases
(`Residence::Released`) the retiring incarnation, so the region count stays depth-independent (`O(1)`)
rather than chaining one region per hop the way a conservative pin would.

- `loop_carried_aggregate_survives_tail_hop_adoption`
- `tail_recursive_record_thread_stays_o1_in_regions`

**TRY-WITH inside TCO position** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — same
`CallFrame::with_scope` seed relocation + bind as MATCH for the per-branch frame; the
`FrameStorage.outer` chain keeps the call-site region alive when the branch body
tail-calls back through the enclosing user-fn.

- `try_inside_tco_position_preserves_frame_chain`

**`KFunction::invoke` per-call frame re-anchor** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — the
seed bind routed through `CallFrame::with_scope`: the deep-cloned argument record is relocated into the
opened child scope's own region through the substrate (the erasing `alloc_object`, which forgets the
caller lifetime) and each parameter bound, while the scope rides the `for<'b>` brand the open confines. Witnessed by the `Rc<CallFrame>`
moved into `BodyResult::Tail`. Exercised by every user-fn invocation: repeated-call reclamation, type-op
dispatch through a functor-call's per-call scope, and `MODULE_TYPE_OF` lift-out.

- `repeated_user_fn_calls_do_not_grow_run_root_per_call`

**Stored scope-pointer re-anchor** ([src/machine/core/scope_ptr.rs](../src/machine/core/scope_ptr.rs)) — every
holder stores a captured / defining / parent scope as a plain `&'a Scope<'a>` (`Module::child_scope`,
`KFunction::captured`, `Scope::outer` / `root`) and re-anchors it **with
the holder as a whole** when the holder is read out of its region (the `Region::alloc` retype in
`witnessed.rs`), so the accessors are bare field reads and scope_ptr.rs carries no `unsafe` of its own.
The construction-time reference is built at `'a` by plain coercion (a same-region child) or at the
construction door's generative brand (a per-call frame child, `build_frame_child_witnessed`) — there is
no construction-time re-anchor verb. This test pins the re-anchor directly through the `Module` carrier;
`KFunction::captured_scope` routes the identical `Region::alloc` retype
(their equivalents run under plain `cargo test`), and every `Scope::outer()` / `ancestors()` walk reads
the field end-to-end.

- `module_child_scope_transmute_does_not_dangle`

**`NodeScope::YokedChild` lifetime fabrication** ([src/machine/execute/nodes.rs](../src/machine/execute/nodes.rs))
— a cart-ancestor block scope evicted off a lifetime-free scheduler node (`NodeScope::YokedChild`) is
stored as a `SealedExtern<ScopeRefFamily>` through the safe `SealedExtern::erase`
(`erase_to_static::<ScopeRefFamily>`) and opened at the read boundary through the `for<'b>`
`SealedExtern::open` — the brand confined to the read, witnessed by the slot's frame `Rc`, sound because
the cart's `outer_frame` chain pins the ancestor region. This is the second lifetime-free scope carrier
(alongside `CallFrame`). This test passes the region as the witness and pins the erase → open round-trip
directly, plus a sibling-pointer region mutation while the opened scope is live.

- `node_scope_yoked_child_erase_open_roundtrip`

**`NodeScope::YokedChild` open — workload read boundary** ([src/machine/execute/dispatch/ctx.rs](../src/machine/execute/dispatch/ctx.rs))
— the `carrier.open(frame, f)` call in the `with_node_scope` helper is the **sole** production
open of a `YokedChild` carrier: it materializes the executing slot's scope from its raw
`NodeScope` handle (the scheduler core hands the handle back but no longer interprets it), passing the
slot's cart `Rc` as the witness to the `for<'b>` `SealedExtern::open` — a **safe** call, no `unsafe`
here. The decide-phase read (`current_scope`, via `SchedulerView`), the Done-boundary post-step read
([src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs)), and the `OwnScope`
re-dispatch (`KoanRuntime::dispatch_in_own_scope` in
[src/machine/execute/runtime/submit.rs](../src/machine/execute/runtime/submit.rs), which clones the
cart `Rc` locally and routes this helper) all funnel through it — none carries an `unsafe` of its own.
It runs the transmute defined in the group above, so `node_scope_yoked_child_erase_open_roundtrip`
— and end-to-end every scheduler-driving slate test — pins it. No separate minimal test.

The `retype` primitive (`Erased<T>` / `Witnessed<T, W>`) and the `ReturnContract`
re-attach it backs at the Done boundary are audited in the `workgraph` crate's own
slate — [workgraph/observe/miri_slate.md](../workgraph/observe/miri_slate.md) — since
their tests live in that crate's lib test binary, a separate `cargo test` target from
koan's. `CarriedFamily`'s `unsafe impl Reattachable`
([src/machine/model/values/carried.rs](../src/machine/model/values/carried.rs)) and this
embedder's `HasRegionHandle` destination operands
([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — over the library's
`RegionSet<FrameStorage>` that `FrameSet` aliases (`FrameStorage` = `RegionHost`, whose `PinsRegion`
lives library-side) — are the Koan-side instantiations that primitive
routes for; `RegionSet::union`'s antichain logic (union with `outer`-chain subsumption) is pinned by
the `frameset_*` / `pins_region_walks_outer_chain` unit tests in
[arena/tests.rs](../src/machine/core/arena/tests.rs), which run under plain `cargo test`.

**`ContinuationFamily` continuation erasure** ([src/machine/execute/outcome.rs](../src/machine/execute/outcome.rs))
— the continuation generalizes the contract discipline from a `ReturnContract` enum to the whole
`NodeContinuation` (`Box<dyn FnOnce>`), as an `Erased<ContinuationFamily>` routing the shared `retype`:
`erase` forgets the captured `'run` for storage on a lifetime-free node, and `SealedExtern::open`
recovers a step brand `'b` witnessed by the slot's start cart `Rc` (which pins the captures' home —
the run region or a strict ancestor of the cart). Distinct shape from the contract group above: the
retype is over a **fat pointer** (data + vtable), not a thin enum, and the carrier is consumed by
value (a non-`Copy` `Box<dyn FnOnce>`), so it carries its own minimal test. The open call site in
[src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs) (`run_step`) runs the same
transmute end-to-end every step. This test pins the erase → open → invoke round-trip directly via
`Erased::erase` + `SealedExtern::open`, calling the opened closure so tree borrows checks the capture
read.

- `erased_continuation_open_roundtrip`

The run-loop step-tail `SealedExtern::open` (`run_step`, opening the continuation, contract, and
consumer `dest` region together at one generative brand) and the doctest fixture markers backing the
`compile_fail` soundness guards are audited in
[workgraph/observe/miri_slate.md](../workgraph/observe/miri_slate.md) alongside the `retype` group they
route through — [src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs)'s and
`finalize.rs`'s call sites carry no `unsafe` of their own.

**`Module` interior mutation under a live `&'a Module`** ([src/machine/model/values/module.rs](../src/machine/model/values/module.rs)) — `Module`
mutates a `RefCell<HashMap>` (`type_members` / `slot_type_tags`) while a `&'a Module<'a>` is
live — the opaque-ascription shape. (The scope re-anchor itself is the stored scope-pointer group
above; the carrier stores a `&'a Scope<'a>`.) The minimal mirror below pins the `borrow_mut`-under-live-`&Module`
shape directly; the end-to-end `opaque_ascription_re_binds_do_not_alias_unsoundly` (which only re-pins the
already-covered `child_scope` re-attach + survives-churn shapes) runs under plain `cargo test`.

- `module_type_members_refcell_mutation_with_held_module_ref`

**MODULE body Combine continuation** ([src/machine/model/values/module.rs](../src/machine/model/values/module.rs)) — the
MODULE body schedules a `Combine` whose `finish` closure captures the child
scope and runs on the outer scheduler's main loop after every body statement
terminalizes. Runs the same stored scope-pointer re-anchor as
`module_child_scope_transmute_does_not_dangle` (the minimal mirror that pins it) with none of its
own, exercised end-to-end by every scheduler-driving slate test; its `module_body_dispatch_does_not_dangle`
program runs under plain `cargo test`. No separate minimal test.

**`NodeStore::reinstall_with_frame` slot re-anchor** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) —
the Replace arm stores the slot's scope as a payload-less `NodeScope::Yoked` marker re-projected
from the frame cart (no fabricated `&'a` persists), so the `Rc<CallFrame>` witness in `Node.frame`
remains the sole liveness root for the re-installed slot's scope.
Exercised by the dispatch-time parking shapes that reinstall through this entry
point (and transitively by user-fn TCO; that path is covered by the MATCH-on-
`Tagged` recursion test above).

- `lift_park_minimal_program_for_miri`
- `replay_park_minimal_program_for_miri`

**`Carried` slot read + dep re-anchor — pinned `open_with`** ([workgraph/src/scheduler/node_store.rs](../workgraph/src/scheduler/node_store.rs))
— the scheduler stores a finalized terminal as a `Witnessed<W::Value, Carrier<W::Frame>>` — the
reference-only carrier, pinning nothing — beside the retention hold finalize seeds, and
`read_result_with` re-anchors under that retained frame owner (`open_with`); a slot with no retained
owner (a drained root re-homed into the run region) is externally pinned, so its read opens under
the empty pin. The consumer-pull dep terminals travel as delivery envelopes — `dep_delivered`
duplicates the slot's envelope per consumer, opened in the consumer `dest` region at `'b`.
`node_store.rs`'s own residual `unsafe` is
only the test-family `Reattachable` markers. Exercised end-to-end by every scheduler-driving program;
the listed test pins the hardest shape — a tail-chain return-type **coarsening** re-homed in the
contract's scope, re-read after the run drains the root into the run region.

- `tail_call_stamps_result_against_first_callers_return_contract`

**`Carried` relocation + escaping-value retention** ([src/machine/execute/lift.rs](../src/machine/execute/lift.rs))
— `relocate_carried` structurally copies a `Carried` into the consumer `dest` region at the brand the
step `open` supplies (a safe alloc — the former value-relocation `unsafe`/fabrication is **deleted**):
the composite spine shares its `Rc` payloads, and a closure / `KFuture` / `Module` rides a *bare*
borrow into its defining region, never copied. That surviving borrow outlives the producer's frame
only because `reached_frame` recovers the region (via the value's scope `region_owner`) and the
consumer frame `retain`s it into `FrameStorage.retained` — at the three read-out boundaries (the
`run_step` relocate, the root drain, and the `extract_terminal` test harness). Safe code; pinned
because tree borrows catches a regression in the retention discipline that would dangle an escaped
closure / module past its per-call frame's drop. The closure shape rides the `KFunction`
captured-scope group above; the test below pins the **module** shape — a functor-minted module
surviving the run that built it.

- `functor_application_is_generative`

**Record escape seam — cost-driven copy vs pin** ([src/machine/execute/lift.rs](../src/machine/execute/lift.rs))
— two distinct seams relocate a top-level `Record` out of a dying producer, each pinned here. The
**container-cell** seam (`copied_seam_mode`, Ruling 4: fresh containers stay self-contained) picks
the per-cell `Residence` a `Residence::Copied` crossing takes: `Released` when
`record_still_borrows_host` ([kobject.rs](../src/machine/model/values/kobject.rs)) finds no surviving
borrow leaf into the cell's own producer host (the record is totally rebuilt via `copy_object_into`
and the producer frees), `Copied` when it does (the producer materializes into the aggregate's reach
and stays pinned). Two unit tests mirror `dispatch::literal::fold_cells`'s exact aggregate loop
(`copied_seam_mode` + `transfer_into_placing` + `copy_held_from_carried`) directly for five
independent producers apiece: one where every record cell is plain data (asserts `Released`, drops
every producer first, then reads every cell back), one where every record cell embeds a closure
captured in that same producer (asserts `Copied`, drops every producer first, then reads every
closure's captured scope back) — a regression in either direction (wrongly releasing a
still-borrowing record, or wrongly pinning a plain one) either dangles under tree borrows or leaks.

The **value-level** escape seam (`seam_verb` → `record_seam_verb`
([kobject.rs](../src/machine/model/values/kobject.rs)), the cost chooser at `relocate_terminal` /
`single_poll` / `finalize`) picks the whole record's `SeamVerb` in O(1) from its memoized copy cost
and borrows-home bit: a **released copy** (`Copy { released: true }` → `Residence::Copied` at the
finalize aggregate) when a priceable plain record is a small fraction of the host's allocated total,
and a **pin** (`SeamVerb::Pin` → `Residence::Kept` + `copy_carried`) when a leaf borrows home — the
record's region-resident substrate rides **shared** (a pointer-copy, never rebuilt), covered by the
Kept-minted producer reach. One end-to-end test drives the released-copy shape through the real
scheduler and parser — a 5-element list literal of user-FN calls each returning a plain-data record
— corroborating the seam is wired to real per-call producer frames; a minimal-shape twin drives the
cost-chooser-selected pin for five independent home-borrowing records (asserts `Pin`, drops every
producer, then reads each closure's captured scope back through the shared substrate), so a
regression that failed to mint the Kept host — or rebuilt instead of sharing — dangles here. Both
verbs are thus UB-audited at the seam under the default cost chooser. The `unsafe` routed is the
shared `retype` in `witnessed.rs`; `lift.rs` carries none of its own (see the file's stale-group
whitelist entry).

- `plain_record_cells_select_released_and_survive_every_producer_free`
- `closure_embedding_record_cells_select_copied_and_pin_every_producer`
- `aggregate_of_plain_record_results_releases_every_producer_frame`
- `record_seam_pin_verb_shares_substrate_and_survives_producer_free`

## Adding tests to the slate

Add a test to the slate when a new unsafe site lands — a transmute, raw-pointer
round-trip, interior-mutation pattern under a live shared borrow, or a cycle
shape that storage-side reasoning can't rule out. Tests are minimal-shape
mirrors of the unsafe operation, not end-to-end feature tests; they fail when
Miri reports UB or a leak, not on values.

When you add or remove a slate test, update the list above (the section
structure mirrors the unsafe-site groupings, so a new test lands under the
group it pins down — or under a new group if it's a new shape) and re-run the
slate to confirm the line count matches.

## Recent full-slate run durations

The five most-recent full-slate runs, newest first. The Miri skill appends a
new entry on every full-slate run and trims to five so this list stays bounded.
Use the most-recent entry as the baseline expectation when scheduling a run.

<!-- slate-durations:start -->
- 2026-07-23: 979s — 44 tests, 0 leaks, 0 UB
- 2026-07-22: 629s — 43 tests, 0 leaks, 0 UB
- 2026-07-22: 475s — 36 tests, 0 leaks, 0 UB
- 2026-07-20: 466s — 36 tests, 0 leaks, 0 UB
- 2026-07-20: 579s — 36 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
