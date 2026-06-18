# Miri audit slate

<!-- slate-fingerprint
src/builtins/test_support.rs: 1
src/machine/core/arena.rs: 12
src/machine/core/kfunction/body.rs: 1
src/machine/core/reattach.rs: 2
src/machine/core/scope_ptr.rs: 5
src/machine/core/storage_frame.rs: 4
src/machine/execute/dispatch/ctx.rs: 1
src/machine/execute/finalize.rs: 1
src/machine/execute/nodes.rs: 1
src/machine/execute/outcome.rs: 4
src/machine/execute/run_loop.rs: 1
src/machine/execute/runtime.rs: 3
src/machine/execute/runtime/submit.rs: 1
src/machine/model/values/carried.rs: 2
src/machine/model/values/module.rs: 1
src/scheduler/erase.rs: 17
src/scheduler/node_store.rs: 4
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

<!-- slate-audit-whitelist:start -->
- `src/machine/core/scope.rs` — `Scope::add` re-entry pins the queue-and-drain
  discipline that keeps `Scope`'s `RefCell<…>` invariant intact when a binding
  is added while a `data` borrow is live.
- `src/machine/core/kfunction.rs` — `KFunction::captured_scope` re-attaches the
  captured scope through the branded `ScopePtr::reattach`, a safe call; the one
  transmute it routes lives in `scope_ptr.rs`, so kfunction.rs carries no `unsafe`
  of its own. The group pins that safe accessor under the closure-escape shape.
<!-- slate-audit-whitelist:end -->

## The slate

25 tests, grouped by the unsafe site each pins down. Names below are the exact
test identifiers; pass them after `--` in the Miri command.

**`CallArena` lifetime erasure** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — the
child-scope `Option<ScopePtr<'static>>` (shortened to an `&self`-bounded lifetime via the
`unsafe` `ScopePtr::reattach_unbounded`) plus the `Rc<CallArena>` chain that keeps per-call
arenas pinned across re-borrow. One test pins the re-attach surviving a sibling alloc; the
other pins the `Rc<CallArena>` chain keeping an outer arena alive after its local handle
drops. A third pins the witness-bounded sibling `ScopePtr::reattach_bounded` (via
`CallArena::scope_bounded`), which splits the stored `'static` into a `&self`-bounded borrow
and a free content lifetime — re-read alongside the unbounded `scope` / `scope_for_bind`
accessors over the same child scope. `CallArena::adopting` (the scheduler-owned run frame)
carries the same `&Scope<'_> → &Scope<'static>` erasure as `new`, over the run scope it adopts
rather than a freshly-minted child; it is built on the first run-lifetime submission, so every
scheduler-driving slate test below (`module_body_dispatch_does_not_dangle`,
`recursive_tagged_match_no_uaf`, `lift_park_minimal_program_for_miri`, …) exercises it
end-to-end — the run scope outlives the frame, so no separate minimal test.

- `call_arena_scope_survives_subsequent_alloc`
- `call_arena_chained_outer_frame_walkable`
- `scope_bounded_reanchors_within_witness_borrow`

**`StorageFrame` alloc engine under live borrows** ([src/machine/core/storage_frame.rs](../src/machine/core/storage_frame.rs)) — the
generic `alloc` engine erases the value to `'static` (the move-through-union `erase_store`),
stores it, records its address into the `membership` `RefCell` via `borrow_mut`, and re-anchors
the `'static` store to `'a` — all while a prior `&` from the same frame is shared-borrowed. Pins
that tree-borrows shape over the engine `RuntimeArena` (= `StorageFrame<KoanStorageProfile>`) routes.

- `runtime_arena_alloc_while_prior_ref_live`

**Cycle gate** ([src/machine/core/storage_frame.rs](../src/machine/core/storage_frame.rs)) — the generic `alloc`
engine redirects a value whose family `anchors_to` answers true for `self` (a self-anchored
`Rc<CallArena>`) to the escape frame via the audited `pin_deref` on the escape pointer, breaking the
storage cycle that closure-escape returns can otherwise produce. The Koan `anchors_to` walkers that drive the
decision live in [src/machine/core/arena.rs](../src/machine/core/arena.rs).

- `alloc_object_redirects_self_anchored_value_to_escape_arena`

**`CallArena::try_reset_for_tail`** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — TCO
frame reuse swaps the inner `RuntimeArena` for a fresh one in place and
re-allocates the child `Scope`, with two new transmutes: `&Scope<'_> →
&Scope<'static>` for the new outer link and a raw-ptr re-anchor for the new
inner arena. The `Rc::get_mut` gate keeps reuse semantically equivalent to
drop-and-alloc by refusing when any other `Rc` to the frame still exists.

- `call_arena_try_reset_for_tail_round_trip`
- `call_arena_try_reset_for_tail_refuses_when_aliased`

**`KFunction` captured-scope re-borrow** ([src/machine/core/kfunction.rs](../src/machine/core/kfunction.rs)) — every
closure invocation reads `KFunction::captured_scope`, a safe call that routes the
branded `BoundedScopePtr::get` on the captured definition-scope pointer (the transmute
lives in `scope_ptr.rs`). The escaped-closure test pins that the pointee outlives the
`KFunction` even when the closure is invoked after its defining frame has returned.

- `closure_escapes_outer_call_and_remains_invocable`

**`Scope::add` re-entry** ([src/machine/core/scope.rs](../src/machine/core/scope.rs)) — adding a binding while
a `data` borrow is live queues onto a pending list and drains on borrow drop,
so the conditional-defer path doesn't violate the `RefCell` invariant. (Safe
code by typestate; pinned in the slate because tree borrows catches the
violation if the queue/drain discipline regresses.)

- `add_during_active_data_borrow_queues_and_drains`

**`USING … SCOPE` transparent-window aliasing** ([src/machine/core/scope.rs](../src/machine/core/scope.rs)) — a
`ScopeBindings::Borrowed` window reads another scope's `RefCell` maps through a
borrowed reference, and the block (run in a transparent scope allocated in the
call-site arena) can define a closure that escapes carrying that window. Pins
that an escaping closure reading a surfaced member of a *temporary* functor-result
module — the harder case, relying on the call-site-arena `Rc` rooting — does not
dangle into the freed module/USING arena. (Safe code by construction; pinned
because tree borrows catches a regression in the aliasing or rooting discipline.)

- `using_temporary_functor_result_is_sound`

**MATCH on `Tagged` recursion** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — MATCH
builds its per-call frame and seeds its `it` bind through `CallArena::with_anchored_child`
(arena re-exposed free, child scope re-handed via the bounded `scope_bounded` brand); the
`outer_frame` chain keeps the call-site arena alive across
TCO replace when a user-fn recurses through a `Tagged` parameter via MATCH.

- `recursive_tagged_match_no_uaf`

**TRY-WITH inside TCO position** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — same
`CallArena::with_anchored_child` seed bind as MATCH for the per-branch frame; the
`outer_frame` chain keeps the call-site arena alive when the branch body
tail-calls back through the enclosing user-fn.

- `try_inside_tco_position_preserves_frame_chain`

**KFuture anchor** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — a KFuture with a
`Future(&KObject)` allocated in the dying arena anchors with `frame: Some(rc)`.
Test source lives in [src/machine/execute/lift.rs](../src/machine/execute/lift.rs);
the unsafe site it pins is the `Rc<CallArena>` heap-pinning that backs the
anchored case (the `frame: None` non-anchor branch is a logic case with no
unsafe site, covered under plain `cargo test`).

- `unanchored_kfuture_with_arena_borrow_does_anchor`

**`KFunction::invoke` per-call frame re-anchor** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — the
seed bind routed through `CallArena::with_anchored_child`: the per-call arena re-exposed at a
free `'a` (an `'a`-typed value must land in an `'a`-typed arena) while the child scope rides the
witness-bounded `scope_bounded` brand. Witnessed by the `Rc<CallArena>` moved into
`BodyResult::Tail`. Exercised by every user-fn invocation: repeated-call reclamation, type-op
dispatch through a functor-call's per-call scope, and `MODULE_TYPE_OF` lift-out.

- `repeated_user_fn_calls_do_not_grow_run_root_per_call`
- `type_op_dispatch_does_not_dangle`

**`ScopePtr` re-attach** ([src/machine/core/scope_ptr.rs](../src/machine/core/scope_ptr.rs)) — the single
`transmute::<&Scope<'static>, &'b Scope<'b>>` (and the `erase` cast) that the unbounded carrier
scope accessors route through. The two carriers that own a real `'a` — `Module::child_scope` and
`Signature::decl_scope` — route the safe `reattach` (the brand makes the call sound);
`CallArena::scope` / `scope_for_bind`, storing a `ScopePtr<'static>`, route the `unsafe`
`reattach_unbounded` to shorten the brand to an `&self`-bounded lifetime. Both paths share the
one transmute. This test pins it directly through the `Module` carrier; the `CallArena` group
exercises the same transmute through its own accessors. `Signature::decl_scope` calls the
identical `reattach` (its line-for-line equivalent runs under plain `cargo test`).
`KFunction::captured_scope` now routes the bounded-twin `BoundedScopePtr::get` (the
`reattach_bounded` transmute, covered by the `BoundedScopePtr` group below), not this
unbounded `reattach`.

- `module_child_scope_transmute_does_not_dangle`

`BoundedScopePtr::{erase, get}` ([src/machine/core/scope_ptr.rs](../src/machine/core/scope_ptr.rs))
are the constraint-free bounded twin used for `Scope::outer`: `erase` is the same raw-ptr cast as
`ScopePtr::erase` (trivially sound, from a reference), and `get` is the **identical**
`transmute::<&'p Scope<'static>, &'p Scope<'a>>` as `ScopePtr::reattach_bounded` — only with a
constraint-free constructor, sound because the free content `'a` is reachable only behind the
`&'p`-bounded re-hand. `get` is exercised by every `Scope::outer()` / `ancestors()` walk, so the
scope-walking shapes already in the slate (and `scope_bounded_reanchors_within_witness_borrow`,
which pins the line-for-line equivalent) cover it; no separate minimal test is added.

**`NodeScope::YokedChild` lifetime fabrication** ([src/machine/execute/nodes.rs](../src/machine/execute/nodes.rs))
— a cart-ancestor block scope evicted off a lifetime-free scheduler node (`NodeScope::YokedChild`) is
stored as a `ScopePtr<'static>` through the brand-dropping `ScopePtr::erase_static` (the same raw-ptr
cast as `erase`, in [src/machine/core/scope_ptr.rs](../src/machine/core/scope_ptr.rs)) and re-attached
at the read boundary through the `unsafe` `ScopePtr::reattach_bounded` — a borrow bounded by the
reader (the slot's frame `Rc`), a free content lifetime, sound because the cart's `outer_frame` chain
pins the ancestor arena. This is the second `'static`-storing scope carrier (alongside `CallArena`).
This test pins the erase → reattach round-trip directly, plus a sibling-pointer arena mutation while
the re-attached scope is live.

- `node_scope_yoked_child_erase_reattach_roundtrip`

**`NodeScope::YokedChild` re-attach — workload read boundary** ([src/machine/execute/dispatch/ctx.rs](../src/machine/execute/dispatch/ctx.rs))
— the `unsafe { ptr.reattach_bounded() }` in the `reattach_node_scope` helper materializes the
executing slot's scope from its raw `NodeScope` handle (the scheduler core hands the handle back but
no longer interprets it). Both the decide-phase read (`current_scope`, via `SchedulerView`) and the
Done-boundary post-step read ([src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs))
route through it. It runs the transmute defined in the group above and carries none of its own, so
`node_scope_yoked_child_erase_reattach_roundtrip` — and end-to-end every scheduler-driving slate test —
pins it. No separate minimal test.

**`NodeScope::YokedChild` re-attach — own-scope re-dispatch** ([src/machine/execute/runtime/submit.rs](../src/machine/execute/runtime/submit.rs))
— the `unsafe { ptr.reattach_bounded() }` in `KoanRuntime::dispatch_in_own_scope` re-dispatches
against a `YokedChild` slot's own scope, running the same transmute with none of its own; pinned by
`node_scope_yoked_child_erase_reattach_roundtrip`. No separate minimal test.

**`retype` primitive — `Erased<T>` / `Reattachable`** ([src/scheduler/erase.rs](../src/scheduler/erase.rs))
— the single audited lifetime-retype every carrier family routes: `retype<A, B>` (a
`transmute_copy` behind a `ManuallyDrop`, the one site `transmute`'s GAT size-proof can't cover),
reached only through `Erased<T>::erase` / `reattach` (stored carriers) and the `reattach_value` /
`reattach_ref` / `reattach_slice` transient helpers. The `unsafe impl Reattachable` families declare
layout-invariance and carry no runtime `unsafe` of their own — they are exercised through this
primitive: `CarriedFamily` / `ResultCarriedFamily`
([src/machine/model/values/carried.rs](../src/machine/model/values/carried.rs)), `ContractFamily`,
`ContFamily`, and `ScopeFamily`. The test erases a borrow-carrying family to the
`'static` store, re-anchors it, and reads through every entry point, re-reading the first borrow
after the helper calls to catch a tree-borrows regression.

- `erased_roundtrip_and_helpers`

**`pin_deref` — raw heap-pin deref** ([src/machine/core/reattach.rs](../src/machine/core/reattach.rs))
— the one audited raw heap-pin deref, materializing a `&'x T` from an `Rc`-pinned `*const T` (the
self-referential arena-pointer derefs the `Erased` retype can't express). Carries no minimal test of
its own: every `CallArena` construction routes it (`CallArena` lifetime erasure /
`try_reset_for_tail` groups), and the storage engine's escape redirect routes it under
`runtime_arena_alloc_while_prior_ref_live`.

**`Reattachable` families — value channel** ([src/machine/model/values/carried.rs](../src/machine/model/values/carried.rs))
— `CarriedFamily` / `ResultCarriedFamily` are `unsafe impl Reattachable` layout-invariance
declarations with no runtime `unsafe` op; the `retype` primitive that consumes them is exercised by
`erased_roundtrip_and_helpers` (and, for `Carried` specifically, every scheduler-driving slate test
through `ErasedValue` / the dep-delivery helpers). No separate minimal test.

**`ErasedContract` re-attach** ([src/machine/core/kfunction/body.rs](../src/machine/core/kfunction/body.rs))
— the contract-lifetime erasure that mirrors `ScopePtr` for `ReturnContract`, now an
`Erased<ContractFamily>` routing the shared `retype` primitive: `erase` forgets the lifetime for
storage on a node's lifetime-free `Frame`, and the `unsafe` `reattach` recovers a lifetime witnessed
by the cart `Rc` that pins the contract's home arena (the cart's frame-outer arena — a strict
ancestor). As a thin-value `Erased` carrier its erase → reattach round-trip is the owned path of the
`retype` primitive, pinned by `erased_roundtrip_and_helpers`; end-to-end, `recursive_tagged_match_no_uaf`
exercises the full carrier through a MATCH arm's `-> :T` carried across tail recursion. No separate
minimal test.

**`ErasedContract` re-attach — Done-boundary call site** ([src/machine/execute/finalize.rs](../src/machine/execute/finalize.rs))
— the `unsafe { contract.reattach() }` in the `NodeFinalize::finalize_terminal` hook routes the
`retype` primitive with none of its own, re-anchoring the contract against the cart held live for the
Done boundary; `erased_roundtrip_and_helpers` (and end-to-end `recursive_tagged_match_no_uaf`) pins
it. No separate minimal test.

**`ErasedCont` continuation erasure** ([src/machine/execute/outcome.rs](../src/machine/execute/outcome.rs))
— the continuation generalizes the `ErasedContract` discipline from a `ReturnContract` enum to the
whole `NodeCont` (`Box<dyn FnOnce>`), as an `Erased<ContFamily>` routing the shared `retype`: `erase`
forgets the captured `'run` for storage on a lifetime-free node, and the `unsafe` `reattach` recovers
a `'run` witnessed by the slot's cart `Rc` (which pins the captures' home — the run arena or a strict
ancestor of the cart). Distinct shape from the contract group above: the retype is over a **fat
pointer** (data + vtable), not a thin enum, so it carries its own minimal test. The re-attach call
site in
[src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs) (the run loop,
just before `run_step`) runs the same transmute end-to-end every step. This test pins the
erase → reattach → invoke round-trip directly, calling the reattached closure so tree borrows checks
the capture read.

- `erased_cont_reattach_roundtrip`

**`ErasedCont` re-attach — run-loop call site** ([src/machine/execute/run_loop.rs](../src/machine/execute/run_loop.rs))
— the `unsafe { erased_cont.reattach(&cart) }` at the top of the execute loop runs the transmute
defined in the group above with none of its own, re-anchoring each slot's continuation against its
cart before `run_step`; the same `erased_cont_reattach_roundtrip` (and end-to-end every
scheduler-driving slate test) pins it. No separate minimal test.

**`Module` interior mutation under a live `&'a Module`** ([src/machine/model/values/module.rs](../src/machine/model/values/module.rs)) — `Module`
mutates a `RefCell<HashMap>` (`type_members` / `slot_type_tags`) while a `&'a Module<'a>` is
live — the opaque-ascription shape. (The scope re-attach itself is the `ScopePtr` group above;
the carriers now store a `ScopePtr`.)

- `module_type_members_refcell_mutation_with_held_module_ref`
- `opaque_ascription_re_binds_do_not_alias_unsoundly`

**MODULE body Combine continuation** ([src/machine/model/values/module.rs](../src/machine/model/values/module.rs)) — the
MODULE body schedules a `Combine` whose `finish` closure captures the child
scope and runs on the outer scheduler's main loop after every body statement
terminalizes. Pins the same `ScopePtr` re-attach site as
`module_child_scope_transmute_does_not_dangle`, exercised end-to-end through
the scheduler path the binder follows.

- `module_body_dispatch_does_not_dangle`

**`NodeStore::reinstall_with_frame` slot re-anchor** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) —
the Replace arm stores the slot's scope as a payload-less `NodeScope::Yoked` marker re-projected
from the frame cart (no fabricated `&'a` persists), so the `Rc<CallArena>` witness in `Node.frame`
remains the sole liveness root for the re-installed slot's scope.
Exercised by the dispatch-time parking shapes that reinstall through this entry
point (and transitively by user-fn TCO; that path is covered by the MATCH-on-
`Tagged` recursion test above).

- `lift_park_minimal_program_for_miri`
- `replay_park_minimal_program_for_miri`

**`Outcome` step-lifetime reattach** ([src/machine/execute/outcome.rs](../src/machine/execute/outcome.rs)) —
the lifetime-only transmutes that remain after the decide surface collapsed to a single cart-scale
lifetime: `deps_at_step` (re-anchors consumer-pull dep terminals to the cart-witnessed lifetime the
continuation runs at) and `pin_carried_to_run` (re-anchors a `'node` read up to `'run` for the
run-global root drain — its sole caller, `interpret.rs`). `Outcome` is single-lifetime, so the
splice slot and dep value share one lifetime — no up/down decide-surface bridge — and a Done terminal
is finalized at the step lifetime within its own step. The `Carried` *storage* erase / read re-anchor itself lives in the
scheduler (`node_store.rs`, group below), not here.
All are exercised by every program; this test pins the hardest shape directly — a tail-chain
return-type **coarsening**, where the re-tagged terminal must be homed in the contract's scope to
outlive the reused producer frame, then re-read after the run drains the root into the run arena.

- `tail_call_stamps_result_against_first_callers_return_contract`

**`Carried` re-attach — scheduler slot read** ([src/scheduler/node_store.rs](../src/scheduler/node_store.rs))
— the scheduler stores a finalized terminal erased to `'static` (`Erased<W::Value>`) and re-anchors
it on read (`read_result` / `read` / `read_result_with_frame`) to the read's own `&self` borrow,
witnessed by the slot's co-stored producer-frame `Rc`: `free_one` / `finalize` need `&mut self`, so
the frame cannot drop while a read borrow is live, so the re-anchored lifetime cannot outlive the
backing arena. The generic `retype` primitive (`erase.rs` group above) does the transmute; these are
its stored-carrier consumers. Exercised end-to-end by every scheduler-driving program — every dep
delivery and top-level read routes a re-anchor — and pinned by
`tail_call_stamps_result_against_first_callers_return_contract`. No separate minimal test.

**`Carried` re-attach — consumer-pull dep lift** ([src/machine/execute/runtime.rs](../src/machine/execute/runtime.rs))
— `KoanRuntime::read_lifted` re-anchors a producer's scheduler read (`'node`) to the destination
*node* lifetime `'o` — the consumer scope's arena, bounded by the active frame `Rc` cloned in
`run_step` — then the `NodeLift` copy relocates it into that arena. Node-to-node, not a `'run`
fabrication: the held producer-frame `Rc` (framed) / the run arena (frameless) pins the read for the
copy, and the lift self-anchors the result via the embedded `Rc`. The `Outcome::Forward` ready path
(`apply_outcome`) routes the same primitive: it pulls the producer terminal through `read_lifted`
into the consumer scope arena, then shortens the node value to the uniform `NodeStep` step lifetime
`'s` (a node→step reattach, the value frame-pinned for all of `'s`). Same `retype` primitive as the
`erase.rs` group. Exercised end-to-end by the lift/park slate tests
(`lift_park_minimal_program_for_miri`, `recursive_tagged_match_no_uaf`, …). No separate minimal test.

**`Carried` re-attach — test-only terminal extraction** ([src/builtins/test_support.rs](../src/builtins/test_support.rs))
— `extract_terminal` widens the scheduler's `'node` read to the scope lifetime for test helpers
(`run_one` / `run_one_type` and peers) that return a top-level result: a frameless terminal living in
the scope arena, which outlives the local scheduler. Test scaffolding, not runtime; exercised under
Miri by every `run_one`-based test. No separate minimal test.

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
- 2026-06-17: 1414s — 25 tests, 0 leaks, 0 UB
- 2026-06-17: 601s — 25 tests, 0 leaks, 0 UB
- 2026-06-17: 609s — 25 tests, 0 leaks, 0 UB
- 2026-06-17: 602s — 25 tests, 0 leaks, 0 UB
- 2026-06-17: 620s — 25 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
