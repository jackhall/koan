# Miri audit slate

<!-- slate-fingerprint
src/machine/core/arena.rs: 4
src/machine/core/scope_ptr.rs: 1
src/machine/model/types/ktype_predicates.rs: 1
src/witnessed.rs: 24
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
- `src/machine/core/kfunction.rs` — `KFunction::captured_scope` re-attaches the
  captured scope through the branded `BoundedScopePtr::get`, a safe call; the one
  transmute it routes lives in `scope_ptr.rs`, so kfunction.rs carries no `unsafe`
  of its own. The group pins that safe accessor under the closure-escape shape.
- `src/machine/model/values/module.rs` — the `Module` groups pin a safe `RefCell`
  discipline (interior mutation under a live `&'a Module`) and the MODULE-body
  Combine continuation; the `BoundedScopePtr::get` re-attach they reference lives in
  `scope_ptr.rs`, so module.rs carries no `unsafe` of its own.
- `src/machine/execute/outcome.rs` — the `ContinuationFamily` group's test
  (`erased_continuation_open_roundtrip`) pins the **fat-pointer** (`Box<dyn>`)
  erase → open → invoke round-trip — a layout shape no thin-carrier test covers.
  The real `unsafe` is the `Erased::reattach` inside `SealedExtern::open` in
  `witnessed.rs`; the family's `unsafe impl` is `reattachable!`-generated, so outcome.rs
  carries none.
- `src/scheduler/node_store.rs` — the slot-read group pins `Witnessed::read` /
  `reattach_with` (the safe borrow-bounded accessors; the `unsafe` lives in
  `witnessed.rs`) via an end-to-end tail-chain return-contract-coarsening shape no
  minimal test reproduces. The file's only former `unsafe` was the test-family markers,
  now `reattachable!`-generated.
- `src/machine/execute/nodes.rs` — `node_scope_yoked_child_erase_reattach_roundtrip`
  pins the `NodeScope::YokedChild` erase → re-attach round-trip plus a sibling-pointer
  region mutation — an `erase_to_static` → `reattach_ref_with` shape through the scope carrier
  that no value-family test reproduces. The re-attach routes the fully-safe
  `ErasedScopePtr::reattach_witnessed` on a stored `&'static Scope`, whose only `unsafe` (the
  shared `retype`) lives in `witnessed.rs`, so nodes.rs carries none of its own.
- `src/machine/execute/dispatch/ctx.rs` — the `reattach_node_scope` read boundary is the
  sole production re-attach of a `YokedChild` pointer; it now passes the executing slot's
  cart `Rc` as the explicit witness to `ErasedScopePtr::reattach_witnessed`, a **safe**
  call, so ctx.rs carries no `unsafe`. The group pins that boundary end-to-end (every
  scheduler-driving slate test); the `unsafe` it routes lives in `witnessed.rs`.
- `src/witnessed/region.rs` — the generic `alloc` engine routes a safe-signature `reattach_ref_with`
  (the only `unsafe` it reaches is the shared `retype` in `witnessed.rs`), so region.rs carries **no
  `unsafe`**. The group pins a safe-code invariant tree borrows can still violate: the alloc engine's
  `membership` `RefCell` `borrow_mut` under a live `&` (`region_alloc_while_prior_ref_live`).
- `src/machine/execute/lift.rs` — `relocate_carried` and `reached_frame` are safe (the value-relocation
  `unsafe` was deleted with the per-value anchor; the copy now allocs at the step brand). The group
  pins the escaping-value **retention** discipline — a surviving closure / module borrow kept alive by
  the consumer frame's `retained` `FrameSet` — which tree borrows catches if it regresses.
<!-- slate-audit-whitelist:end -->

## The slate

36 tests, grouped by the unsafe site each pins down. Names below are the exact
test identifiers; pass them after `--` in the Miri command.

**`CallFrame` lifetime erasure** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — the
child-scope `Option<SealedExtern<ScopeRefFamily>>` (re-attached to an `&self`-bounded borrow via the
witness-bounded `SealedExtern::attach`, the frame's own storage `Rc` as the pin) plus
the `Rc<CallFrame>` chain that keeps per-call regions pinned across re-borrow. One test pins the
re-attach surviving a sibling alloc; the other pins the `Rc<CallFrame>` chain keeping an outer region
alive after its local handle drops. A third pins `SealedExtern::attach` (via
`CallFrame::scope_bounded`), which splits the stored `&'static Scope` into a witness-bounded borrow
and a free content lifetime — re-read alongside the collapsed `scope` / `scope_for_bind`
accessors (`'b = 'step`) over the same child scope. `CallFrame::adopting` (the scheduler-owned run frame)
carries the same `&Scope<'_>` erasure as `new`, over the run scope it adopts
rather than a freshly-minted child; it is built on the first run-lifetime submission, so every
scheduler-driving slate test below (`recursive_tagged_match_no_uaf`,
`lift_park_minimal_program_for_miri`, …) exercises it
end-to-end — the run scope outlives the frame, so no separate minimal test.

- `call_frame_scope_survives_subsequent_alloc`
- `call_frame_chained_outer_frame_walkable`
- `scope_bounded_reanchors_within_witness_borrow`

**`Region` alloc engine under live borrows** ([src/witnessed/region.rs](../src/witnessed/region.rs)) — the
generic `alloc` engine erases the value to `'static` (the move-through-union `erase_store`),
stores it, records its address into the `membership` `RefCell` via `borrow_mut`, and re-anchors
the `'static` store to `'a` through the witness-bounded `reattach_ref_with` (the region itself as the
pin, a **safe** call) — all while a prior `&` from the same frame is shared-borrowed. Pins that
tree-borrows shape over the engine `KoanRegion` (= `Region<KoanStorageProfile>`) routes.

- `region_alloc_while_prior_ref_live`

**`CallFrame::try_reset_for_tail`** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — TCO
frame reuse installs a fresh refcounted `FrameStorage` (a new `KoanRegion`) and
re-allocates the child `Scope` through the safe `Scope::child_for_frame`: the new
outer link and root are brand-shortened to the fresh region's lifetime, so the
child is built at real lifetimes and erased once via `SealedExtern::erase` with
no construction-time transmute. The re-attach these tests pin is the read-side
`SealedExtern::attach` on the re-installed child plus the swap's drop
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
closure invocation reads `KFunction::captured_scope`, a safe call that routes the
branded `BoundedScopePtr::get` on the captured definition-scope pointer (the transmute
lives in `scope_ptr.rs`). The escaped-closure test pins that the pointee outlives the
`KFunction` even when the closure is invoked after its defining frame has returned.

- `fast_lane_closure_escapes_outer_call_and_remains_invocable`

**`Scope::add` re-entry** ([src/machine/core/scope.rs](../src/machine/core/scope.rs)) — adding a binding while
a `data` borrow is live queues onto a pending list and drains on borrow drop,
so the conditional-defer path doesn't violate the `RefCell` invariant. (Safe
code by typestate; pinned in the slate because tree borrows catches the
violation if the queue/drain discipline regresses.)

- `add_during_active_data_borrow_queues_and_drains`

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
builds its per-call frame and seeds its `it` bind through `CallFrame::with_frame_interior`
(the region reached through the child scope's `region` field, the scope re-handed via the bounded
`scope_bounded` brand); the `FrameStorage` ancestor chain keeps the call-site region alive across
TCO replace when a user-fn recurses through a `Tagged` parameter via MATCH.

- `recursive_tagged_match_no_uaf`

**TRY-WITH inside TCO position** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — same
`CallFrame::with_frame_interior` seed bind as MATCH for the per-branch frame; the
`FrameStorage.outer` chain keeps the call-site region alive when the branch body
tail-calls back through the enclosing user-fn.

- `try_inside_tco_position_preserves_frame_chain`

**`KFunction::invoke` per-call frame re-anchor** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — the
seed bind routed through `CallFrame::with_frame_interior`: the per-call region reached through the
child scope's `region` field at the scope's content `'a` (an `'a`-typed value must land in an
`'a`-typed region) while the child scope rides the witness-bounded `scope_bounded` brand. Witnessed
by the `Rc<CallFrame>` moved into
`BodyResult::Tail`. Exercised by every user-fn invocation: repeated-call reclamation, type-op
dispatch through a functor-call's per-call scope, and `MODULE_TYPE_OF` lift-out.

- `repeated_user_fn_calls_do_not_grow_run_root_per_call`

**`BoundedScopePtr::get` re-attach** ([src/machine/core/scope_ptr.rs](../src/machine/core/scope_ptr.rs)) — the
`transmute::<&'p Scope<'static>, &'p Scope<'a>>` (and the `erase` cast) the **safe** branded
scope handle routes. The carriers that own a real `'a` — `Module::child_scope`,
`Signature::decl_scope`, `KFunction::captured_scope`, and `Scope::outer` — all re-hand through
`BoundedScopePtr::get`: the brand records the content `'a`, and the re-hand is reader-bounded, so the
free content is never cashed unbounded and the call is sound *without* `unsafe`. This test pins the
transmute directly through the `Module` carrier; `Signature::decl_scope` / `KFunction::captured_scope`
call the identical `get` (their line-for-line equivalents run under plain `cargo test`), and every
`Scope::outer()` / `ancestors()` walk exercises it end-to-end.

- `module_child_scope_transmute_does_not_dangle`

**`KType::accepts_part` lifetime coercion** ([src/machine/model/types/ktype_predicates.rs](../src/machine/model/types/ktype_predicates.rs))
— the read-only `transmute::<&ExpressionPart<'e>, &ExpressionPart<'a>>` at `accepts_part`'s entry,
coercing a `'b`-branded part to the type's lifetime so the dispatch-resolution decouple (threading a
scope at an independent `'b`) can structurally admit it. Sound because the predicate only *reads* the
part — no mutation, no borrow escapes — and the part outlives the call. Interim until a
lifetime-agnostic `KType` equality lands (the structural-value-equality roadmap item).

- `accepts_part_lifetime_coercion_reads_soundly`

**Witness-bounded scope re-attach — `SealedExtern::attach` / `ErasedScopePtr::reattach_witnessed`** ([src/machine/core/scope_ptr.rs](../src/machine/core/scope_ptr.rs))
— the scope-pointer analog of `reattach_with`: the two lifetime-free scope carriers re-anchor their
child scope through a **fully safe** accessor that takes the pinning `Rc`/region as an explicit
`Witness` borrow. Each holds a `&'static Scope` (erased once on the store side through the safe
`erase_to_static::<ScopeRefFamily>`), so the re-hand is the witnessed `reattach_ref_with::<ScopeFamily>`
on that stored reference with **no `unsafe`** of its own — the only `unsafe` it routes is the shared
`retype` in `witnessed.rs`. The per-call frame's child scope rides the substrate's
`SealedExtern<ScopeRefFamily>` and re-anchors through `SealedExtern::attach` (`CallFrame::scope` /
`scope_for_bind` / `scope_bounded` pass the frame's own storage `Rc`); a scheduler node's
`NodeScope::YokedChild` rides an `ErasedScopePtr` and re-anchors through `ErasedScopePtr::reattach_witnessed`
(passing the slot's cart `Rc`). Both yield a borrow bounded by the witness with a free content lifetime,
sound because the external witness (the frame `Rc`, which for a `YokedChild` pins the ancestor region via
`FrameStorage.outer`) keeps the pointee live for the borrow. Call sites carry **no `unsafe`**. The store
side (`SealedExtern::erase` / `ErasedScopePtr::erase`) forgets the reference's lifetime for storage
through the safe `erase_to_static`. The `CallFrame` group exercises `attach` through its own accessors;
the YokedChild groups below pin `reattach_witnessed` through the node carrier.

**`NodeScope::YokedChild` lifetime fabrication** ([src/machine/execute/nodes.rs](../src/machine/execute/nodes.rs))
— a cart-ancestor block scope evicted off a lifetime-free scheduler node (`NodeScope::YokedChild`) is
stored as an `ErasedScopePtr` through `ErasedScopePtr::erase` (the raw-ptr cast in
[src/machine/core/scope_ptr.rs](../src/machine/core/scope_ptr.rs)) and re-attached at the read boundary
through the safe-signature `ErasedScopePtr::reattach_witnessed` — the borrow bounded by the witness
(the slot's frame `Rc`), a free content lifetime, sound because the cart's `outer_frame` chain pins the
ancestor region. This is the second lifetime-free scope carrier (alongside `CallFrame`). This test
passes the region as the witness and pins the erase → reattach round-trip directly, plus a
sibling-pointer region mutation while the re-attached scope is live.

- `node_scope_yoked_child_erase_reattach_roundtrip`

**`NodeScope::YokedChild` re-attach — workload read boundary** ([src/machine/execute/dispatch/ctx.rs](../src/machine/execute/dispatch/ctx.rs))
— the `ptr.reattach_witnessed(frame)` call in the `reattach_node_scope` helper is the **sole** production
re-attach of a `YokedChild` pointer: it materializes the executing slot's scope from its raw
`NodeScope` handle (the scheduler core hands the handle back but no longer interprets it), passing the
slot's cart `Rc` as the explicit witness — a **safe** call, no `unsafe` here. The
decide-phase read (`current_scope`, via `SchedulerView`), the Done-boundary post-step read
([src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs)), and the `OwnScope`
re-dispatch (`KoanRuntime::dispatch_in_own_scope` in
[src/machine/execute/runtime/submit.rs](../src/machine/execute/runtime/submit.rs), which clones the
cart `Rc` locally and routes this helper) all funnel through it — none carries an `unsafe` of its own.
It runs the transmute defined in the group above, so `node_scope_yoked_child_erase_reattach_roundtrip`
— and end-to-end every scheduler-driving slate test — pins it. No separate minimal test.

**`retype` primitive — `Erased<T>` / `Witnessed<T, W>`** ([src/witnessed.rs](../src/witnessed.rs))
— the single audited lifetime-retype every carrier family routes: `retype<A, B>` (a
`transmute_copy` behind a `ManuallyDrop`, the one site `transmute`'s GAT size-proof can't cover),
reached through `Erased<T>::erase` / `reattach`, the witness-borrowed `reattach_with` /
`reattach_ref_with` helpers, the `reattach_ref` transient helper, the
consuming externally-witnessed `SealedExtern::open` (which reattaches a witness-less carrier — or a
`zip`-combined product / `seal_option` optional of carriers — at a generative `for<'b>` brand the
supplied witness pins), and through the `Witnessed` accessors: the rank-2 branded `with`
(borrow + read) and `map` (consume + transform), the borrow-bounded `read` that hands the carrier
*out* at the `&self` borrow — sound because its content lifetime is the borrow itself (not a free
`'b`), so the bundled `Witness` pins it for exactly that long — and the rank-2 branded `merge`, which
re-anchors *two* carriers under one `'b`, runs a binding projection, and re-seals under the
descendant witness (the one whose ancestor-chain pin keeps both regions live), rejecting unrelated
carts. The co-location-enforcing constructor `yoke` sources its carrier from the witness's region
through a `for<'b>` closure (no `unsafe` of its own — it routes the safe `erase`), so it is exercised
for the brand discipline, not a retype. The `unsafe impl Reattachable` families declare
layout-invariance and carry no runtime `unsafe` of their own — they are exercised through this
primitive: `CarriedFamily`
([src/machine/model/values/carried.rs](../src/machine/model/values/carried.rs)), `ContractFamily`,
`ContFamily`, `ScopeFamily`, the `BoxFamily` non-`Copy` stand-in, and the generic `And` product /
`OptionOf` optional families the `zip` / `seal_option` combinators seal. The tests erase a
borrow-carrying family to the `'static` store and
re-anchor it through every entry point — the witness-less helpers, the borrow-bounded `read` (read
after the original binding drops), and the `Witnessed` accessors that drop the *original* binding and
read back only through the bundled witness (the load-bearing case for the invariant `Cell<&'r u32>`
carrier) — plus `map`'s branded projection (binding a cart-coherent `&'b` value into the invariant
scope slot, the write `with` rejects). `yoke` sources a carrier from a stand-in cart's region, and
`merge` binds an ancestor-cart ref into a descendant-cart scope at the shared brand and re-seals under
the descendant (read back after both call handles drop), plus a `None`-on-unrelated-carts check.
`SealedExtern::open` is exercised distinctly from the bundled `with` / `read`: a witness-less carrier
opened against a *separately-held* `Rc` witness (invariant `Cell<&'r u32>` read back after the
original drops), a **non-`Copy`** `Box<&'r u32>` consumed by the open (the boxed-continuation shape
`Copy`-bounded `Sealed::open` excludes), and a heterogeneous `zip` of a boxed carrier + a present
`seal_option` optional + a reference opened together at one brand (plus the `None`-optional arm). The
escape-can't-compile guards are `compile_fail` doctests on `with` / `map` / `yoke` / `merge` /
`SealedExtern::open`.

The **production** realisation of these `unsafe trait` impls — `Witness` / `WitnessRegion` /
`MergeWitness` for `FrameSet`, the unified region-owner witness in
[src/machine/core/arena.rs](../src/machine/core/arena.rs) — is covered here cross-file: its
region-plus-`outer`-ancestry shape is exactly what the `Rc<TestCart>` stand-in mirrors, so
`yoke_sources_carrier_from_witness_region` and `merge_binds_ancestor_ref_into_descendant_scope`
pin its yoke / merge / subsumption (drop-an-ancestor-still-pinned-by-the-chain) UB shapes, and
`merge_rejects_unrelated_carts` the no-common-pin verdict. `FrameSet::merge`'s antichain logic
(union with `outer`-chain subsumption) is pinned by the `frameset_*` / `pins_region_walks_outer_chain`
unit tests in [arena/tests.rs](../src/machine/core/arena/tests.rs).

- `erased_roundtrip_and_helpers`
- `read_borrow_bounded_witness_only`
- `reattach_with_live_value_and_ref`
- `covariant_roundtrip_witness_only`
- `invariant_roundtrip_witness_only`
- `continuation_binds_cart_coherent_value_via_map`
- `invariant_same_brand_mutation`
- `yoke_sources_carrier_from_witness_region`
- `merge_binds_ancestor_ref_into_descendant_scope`
- `merge_rejects_unrelated_carts`
- `sealed_extern_open_externally_witnessed`
- `sealed_extern_open_consumes_non_copy`
- `sealed_extern_zip_opens_heterogeneous_at_one_brand`
- `seal_option_none_opens_to_none`

**`ReturnContract` re-attach — Done-boundary open** ([src/witnessed.rs](../src/witnessed.rs))
— the contract opens at the run-loop step brand alongside the continuation (a `seal_option` optional
operand of the step's `SealedExtern::open`), so it is a live `ReturnContract<'b>` at the Done arm with
no reattach in `finalize.rs`; the `unsafe` lives in `SealedExtern::open` (`Erased::reattach`). The
start cart pins the contract's home region (a strict ancestor of the producer frame).
`erased_roundtrip_and_helpers` / `sealed_extern_zip_opens_heterogeneous_at_one_brand` (and end-to-end
`recursive_tagged_match_no_uaf`) pin it. No separate minimal test.

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

**`SealedExtern::open` — run-loop step-tail open** ([src/witnessed.rs](../src/witnessed.rs))
— the `unsafe { self.value.reattach() }` inside `SealedExtern::open` runs the transmute defined in the
`retype` group above with none of its own, opening the run-loop step's continuation, contract, and
consumer `dest` region together at one generative `for<'b>` brand the start cart pins (`run_step`); the
dep slice and an `Outcome::Forward` pull are then born at `'b` from the opened region. The
`sealed_extern_*` minimal tests above and end-to-end every scheduler-driving slate test pin it. The
`run_loop.rs` / `finalize.rs` call sites carry no `unsafe` of their own (the `open` brand confines the
reattach). No separate minimal test beyond the `retype` group's.

**`Module` interior mutation under a live `&'a Module`** ([src/machine/model/values/module.rs](../src/machine/model/values/module.rs)) — `Module`
mutates a `RefCell<HashMap>` (`type_members` / `slot_type_tags`) while a `&'a Module<'a>` is
live — the opaque-ascription shape. (The scope re-attach itself is the `BoundedScopePtr::get` group
above; the carrier stores a `BoundedScopePtr<'a>`.) The minimal mirror below pins the `borrow_mut`-under-live-`&Module`
shape directly; the end-to-end `opaque_ascription_re_binds_do_not_alias_unsoundly` (which only re-pins the
already-covered `child_scope` re-attach + survives-churn shapes) runs under plain `cargo test`.

- `module_type_members_refcell_mutation_with_held_module_ref`

**MODULE body Combine continuation** ([src/machine/model/values/module.rs](../src/machine/model/values/module.rs)) — the
MODULE body schedules a `Combine` whose `finish` closure captures the child
scope and runs on the outer scheduler's main loop after every body statement
terminalizes. Runs the same `BoundedScopePtr::get` re-attach site as
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

**`Carried` slot read + dep re-anchor — `Witnessed::read` / `reattach_with`** ([src/scheduler/node_store.rs](../src/scheduler/node_store.rs))
— the scheduler stores a finalized terminal as a `Witnessed<W::Value, Option<Rc<W::Cart>>>` bundling
the erased value with its producer-frame `Rc`, and `read_result` / `read` / `read_result_with_frame`
hand it back through the **safe** `Witnessed::read` (the borrow-bounded accessor in the `witnessed`
group above): `free_one` / `finalize` need `&mut self`, so the frame cannot drop while a read borrow
is live, so the re-anchored lifetime cannot outlive the backing region. The consumer-pull dep
terminals are born at the step brand directly — `read_lifted` lifts each into the consumer `dest`
region opened at `'b` (the frameless arm a safe `reattach_with`), so no separate slice re-anchor.
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
- 2026-06-27: 117s — 36 tests, 0 leaks, 0 UB
- 2026-06-27: 123s — 36 tests, 0 leaks, 0 UB
- 2026-06-27: 157s — 36 tests, 0 leaks, 0 UB
- 2026-06-26: 159s — 36 tests, 0 leaks, 0 UB
- 2026-06-26: 134s — 36 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
