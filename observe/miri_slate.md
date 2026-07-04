# Miri audit slate

<!-- slate-fingerprint
src/machine/core/arena.rs: 2
src/machine/core/scope.rs: 1
src/machine/model/types/ktype_predicates.rs: 2
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
- `workgraph/src/scheduler/node_store.rs` — the slot-read group pins `Witnessed::read`
  (the safe borrow-bounded accessor; the `unsafe` lives in
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
- `src/machine/execute/lift.rs` — `relocate_carried` and `reached_frame` are safe (the value-relocation
  `unsafe` was deleted with the per-value anchor; the copy now allocs at the step brand). The group
  pins the escaping-value **retention** discipline — a surviving closure / module borrow kept alive by
  the consumer frame's `retained` `FrameSet` — which tree borrows catches if it regresses.
<!-- slate-audit-whitelist:end -->

## The slate

32 tests, grouped by the unsafe site each pins down. Names below are the exact
test identifiers; pass them after `--` in the Miri command. A further 14 tests
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

**Empty-witness transient — foreign-reach-only alloc** ([src/machine/core/arena.rs](../src/machine/core/arena.rs))
— a region-pure object allocated through the brand-confined `alloc_object_witnessed` is born under
`FrameSet::empty()` (its *foreign* reach — the active frame excluded), so its witness pins **nothing**.
Sound only as a within-step transient: the active frame pins the region externally for the construction
step, and `finalize` folds the producer into the carrier's witness (`Witnessed::reseal_under`) **before**
the carrier is stored on a node. The test pins that fold-before-store across a TCO reset — fold the
producer, seal, then `try_reset_for_tail` the producer *shell*; the folded producer-storage pin keeps
the pre-reset region (where the value lives) alive, so opening the sealed carrier after the reset reads
a live pointee. Without the fold the empty witness would pin nothing and the reset would free the region
under the stored carrier. The only `unsafe` it routes is the shared `retype` in `witnessed.rs` (through
`Sealed::open` and `reseal_under`'s `merge`).

- `empty_witness_carrier_survives_producer_shell_reset_after_fold`

**Honest single-region witness — multi-region union** ([src/machine/core/arena.rs](../src/machine/core/arena.rs))
— the single-region `yoke` seam is `WitnessRegion for Rc<FrameStorage>` (a held owner pins exactly its
own region — a compile-time type, not `FrameSet::region`'s panicking `.first()` narrowing), and a
freshly-yoked carrier lifts to the aggregate `FrameSet` through `SetWitness for FrameSet`
(`Witnessed::into_set`). These tests hand-build genuinely multi-region carriers — a value reaching
several *independently-dying* per-call regions — through the born-witnessed verbs only (`resident` +
`reseal_under` for a region-pure closure leaf, `yoke_branded` + `transfer_into` / `merge` to derive the
reach union, never a hand-assembled witness), free every producing frame, then read a reached closure's
captured scope back: a use-after-free under tree borrows the instant the witness under-counts (a single
frame witnessing the whole aggregate frees the others' regions). The three shapes are the design's
multi-region cases — a list of closures, a closure capturing closures (the reach tree), and a record
whose fields reach distinct regions. The only `unsafe` routed is the shared `retype` in `witnessed.rs`
(through `yoke` / `merge` / `map` / `reseal_under` / `with`); the `WitnessRegion` / `SetWitness` impls
assert only their region-pin contracts.

- `multi_region_list_of_closures_survives_frame_free`
- `multi_region_closure_capturing_closures_survives_frame_free`
- `multi_region_record_of_closures_survives_frame_free`

**`CallFrame::try_reset_for_tail`** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — TCO
frame reuse installs a fresh refcounted `FrameStorage` (a new `KoanRegion`) and
re-allocates the child `Scope` through the externally-witnessed construction door
(`build_frame_child_witnessed`): the new outer link and root are brand-shortened to the fresh region's
lifetime *by the door's generative brand*, so the child is built at real lifetimes and erased once via
`SealedExtern::erase` with no construction-time transmute of its own. The read these tests pin is `CallFrame::with_scope`
(`SealedExtern::open`) on the re-installed child plus the swap's drop
discipline: the `Rc::get_mut` gate refuses only when another `Rc<CallFrame>`
*shell* holder still exists; an escaped value pins the `FrameStorage`, not the
shell, so it does not foreclose reuse — the swap drops the shell's reference to the
old storage while the escapee's clone keeps that snapshot alive and aliased. The
carrier bundles no `Rc` clone (it holds a `&'static Scope`), so it does not peg the
`Rc::get_mut` uniqueness check the reset depends on.

- `call_frame_try_reset_for_tail_round_trip`
- `call_frame_try_reset_for_tail_refuses_when_aliased`
- `call_frame_try_reset_for_tail_allows_reset_under_escaped_storage`

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
lifetime (`Erased::reattach` to `'a`), copy-free, pinned by the reach-set `fold_reach` deposits
**before** the reattach. This test seals a value witnessed by a producer frame, adopts it into a
consumer scope in a *different* frame, drops every direct producer handle, then reads the adopted
value — so the folded reach-set is the sole pin on the region the re-anchored borrow reads, and tree
borrows catches a use-after-free if the fold-then-reanchor order or the pin regresses.

- `adopt_sealed_reach_fold_pins_the_producer_region_after_drop`

**`USING … SCOPE` transparent-window aliasing** ([src/machine/core/scope.rs](../src/machine/core/scope.rs)) — a
`ScopeBindings::Borrowed` window reads another scope's `RefCell` maps through a
borrowed reference, and the block (run in a transparent scope allocated in the
call-site region) can define a closure that escapes carrying that window. Pins
that an escaping closure reading a surfaced member of a *temporary* functor-result
module — the harder case, relying on the call-site-region `Rc` rooting — does not
dangle into the freed module/USING region. (Safe code by construction; pinned
because tree borrows catches a regression in the aliasing or rooting discipline.)

- `using_temporary_functor_result_is_sound`

**MATCH on `Tagged` recursion** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — MATCH
builds its per-call frame and seeds its `it` bind through `CallFrame::with_scope`: the matched value,
deep-cloned at the caller lifetime, is relocated into the opened child scope's own region through the
substrate (the erasing `alloc_object`, which forgets the caller lifetime) and bound; the `FrameStorage` ancestor chain keeps the
call-site region alive across TCO replace when a user-fn recurses through a `Tagged` parameter via
MATCH.

- `recursive_tagged_match_no_uaf`

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
`ModuleSignature::decl_scope`, `KFunction::captured`, `Scope::outer` / `root`) and re-anchors it **with
the holder as a whole** when the holder is read out of its region (the `Region::alloc` retype in
`witnessed.rs`), so the accessors are bare field reads and scope_ptr.rs carries no `unsafe` of its own.
The construction-time reference is built at `'a` by plain coercion (a same-region child) or at the
construction door's generative brand (a per-call frame child, `build_frame_child_witnessed`) — there is
no construction-time re-anchor verb. This test pins the re-anchor directly through the `Module` carrier;
`ModuleSignature::decl_scope` / `KFunction::captured_scope` route the identical `Region::alloc` retype
(their equivalents run under plain `cargo test`), and every `Scope::outer()` / `ancestors()` walk reads
the field end-to-end.

- `module_child_scope_transmute_does_not_dangle`

**`KType::accepts_part` / `accepts_cell` lifetime coercion** ([src/machine/model/types/ktype_predicates.rs](../src/machine/model/types/ktype_predicates.rs))
— two read-only lifetime coercions for the same structural-admission purpose. `accepts_part`'s entry
`transmute::<&ExpressionPart<'e>, &ExpressionPart<'a>>` coerces a `'b`-branded part to the type's
lifetime; `accepts_cell` opens a spliced cell and `transmute::<Carried<'b>, Carried<'a>>`s the value
to the slot's lifetime for the same-lifetime `accepts_carried`. Both are sound because the predicate
only *reads* — no mutation, no borrow escapes (only a `bool`) — and the value outlives the call.
Interim until a lifetime-agnostic `KType` equality lands (the structural-value-equality roadmap item).

- `accepts_part_lifetime_coercion_reads_soundly`
- `spliced_cell_classifies_like_bare_splice`

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
embedder's `Witness` / `WitnessRegion` / `PinsRegion` for `FrameStorage`
([src/machine/core/arena.rs](../src/machine/core/arena.rs)), backing the library's
`RegionSet<FrameStorage>` that `FrameSet` aliases, are the Koan-side instantiations that primitive
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

**`Carried` slot read + dep re-anchor — `Witnessed::read`** ([workgraph/src/scheduler/node_store.rs](../workgraph/src/scheduler/node_store.rs))
— the scheduler stores a finalized terminal as a `Witnessed<W::Value, Option<Rc<W::Cart>>>` bundling
the erased value with its producer-frame `Rc`, and `read_result` / `read` / `read_result_with_frame`
hand it back through the **safe** `Witnessed::read` (the borrow-bounded accessor in the `witnessed`
group above): `free_one` / `finalize` need `&mut self`, so the frame cannot drop while a read borrow
is live, so the re-anchored lifetime cannot outlive the backing region. The consumer-pull dep
terminals are born at the step brand directly — `read_lifted` lifts each into the consumer `dest`
region opened at `'b`, so no separate slice re-anchor.
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
- 2026-07-03: 142s — 30 tests, 0 leaks, 0 UB
- 2026-07-03: 143s — 30 tests, 0 leaks, 0 UB
- 2026-07-03: 141s — 30 tests, 0 leaks, 0 UB
- 2026-07-03: 146s — 44 tests, 0 leaks, 0 UB
- 2026-07-02: 157s — 44 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
