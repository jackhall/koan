# Miri audit slate

<!-- slate-fingerprint
src/builtins/match_case.rs: 2
src/builtins/try_with.rs: 2
src/machine/core/arena.rs: 23
src/machine/core/kfunction.rs: 1
src/machine/core/kfunction/invoke.rs: 1
src/machine/execute/scheduler/node_store.rs: 1
src/machine/model/ast.rs: 1
src/machine/model/values/module.rs: 3
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
<!-- slate-audit-whitelist:end -->

## The slate

30 tests, grouped by the unsafe site each pins down. Names below are the exact
test identifiers; pass them after `--` in the Miri command.

**Singleton transmutes** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — the `'static`→`'a`
re-annotation on the `NULL_HOLDER` / `TRUE_HOLDER` / `FALSE_HOLDER` shared singletons.

- `singleton_ref_independent_of_arena_lifetime`
- `singletons_aliasable`

**`CallArena` lifetime erasure** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — the
`*const Scope<'static>` round-trip plus the `Rc<CallArena>` chain that keeps
per-call arenas pinned across re-borrow.

- `call_arena_scope_survives_subsequent_alloc`
- `call_arena_scope_survives_subsequent_alloc_via_raw_ptr_roundtrip`
- `call_arena_scope_repeated_calls_alias`
- `call_arena_chained_outer_frame_walkable`
- `call_arena_scope_re_anchored_into_struct_alongside_rc`

**`RuntimeArena` interior mutation under live borrows** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)).

- `runtime_arena_alloc_while_prior_ref_live`

**Cycle gate** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — `alloc_object` redirects
a value carrying a self-anchored `Rc<CallArena>` to the escape arena, breaking
the storage cycle that closure-escape returns can otherwise produce.

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
closure invocation reads `KFunction::captured_scope`, which is `NonNull::as_ref`
on the captured definition-scope pointer. The escaped-closure tests pin that
the pointee outlives the `KFunction` even when the closure is invoked after its
defining frame has returned.

- `closure_escapes_outer_call_and_remains_invocable`
- `escaped_closure_with_param_returns_body_value`

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
that an escaping closure reading a surfaced member of a functor-result module —
bound or temporary, the latter relying on the call-site-arena `Rc` rooting —
does not dangle into the freed module/USING arena. (Safe code by construction;
pinned because tree borrows catches a regression in the aliasing or rooting
discipline.)

- `using_functor_result_closure_escapes_soundly`
- `using_temporary_functor_result_is_sound`

**MATCH on `Tagged` recursion** ([src/builtins/match_case.rs](../src/builtins/match_case.rs)) — the
`outer_frame` chain keeps the call-site arena alive across TCO replace when a
user-fn recurses through a `Tagged` parameter via MATCH.

- `recursive_tagged_match_no_uaf`

**TRY-WITH inside TCO position** ([src/builtins/try_with.rs](../src/builtins/try_with.rs)) — same
`(inner_arena, child)` re-anchor as MATCH for the per-branch frame; the
`outer_frame` chain keeps the call-site arena alive when the branch body
tail-calls back through the enclosing user-fn.

- `try_inside_tco_position_preserves_frame_chain`

**KFuture anchor decision** ([src/machine/core/arena.rs](../src/machine/core/arena.rs)) — the targeted anchor: a KFuture
whose descendants don't borrow into the dying arena lifts with `frame: None`;
one with a `Future(&KObject)` allocated in the dying arena anchors with
`frame: Some(rc)`. Test source lives in [src/machine/execute/lift.rs](../src/machine/execute/lift.rs);
the unsafe site it pins is the `Rc<CallArena>` heap-pinning that backs the
anchored case.

- `unanchored_kfuture_no_arena_borrow_does_not_anchor`
- `unanchored_kfuture_with_arena_borrow_does_anchor`

**`KFunction::invoke` per-call frame transmute** ([src/machine/core/kfunction/invoke.rs](../src/machine/core/kfunction/invoke.rs)) — the
consolidated `(inner_arena, child): (&'a RuntimeArena, &'a Scope<'a>)` re-anchor
that lifts the per-call frame's receiver-bound borrows to the outer slot-storage
lifetime. Witnessed by the `Rc<CallArena>` moved into `BodyResult::Tail`.
Exercised by every user-fn invocation: repeated-call reclamation, type-op
dispatch through a functor-call's per-call scope, and `MODULE_TYPE_OF` lift-out.

- `repeated_user_fn_calls_do_not_grow_run_root_per_call`
- `type_op_dispatch_does_not_dangle`

**Module / Signature lifetime erasure** ([src/machine/model/values/module.rs](../src/machine/model/values/module.rs)) — `Module`
and `Signature` carry their captured scope as `*const Scope<'static>` and
re-attach `'a` via transmute on access; `Module::type_members` mutates a
`RefCell<HashMap>` while a `&'a Module<'a>` is live (the opaque-ascription
shape).

- `module_child_scope_transmute_does_not_dangle`
- `signature_decl_scope_transmute_does_not_dangle`
- `module_type_members_refcell_mutation_with_held_module_ref`
- `opaque_ascription_re_binds_do_not_alias_unsoundly`

**MODULE body Combine continuation** ([src/machine/model/values/module.rs](../src/machine/model/values/module.rs)) — the
MODULE body schedules a `Combine` whose `finish` closure captures the child
scope and runs on the outer scheduler's main loop after every body statement
terminalizes. Pins the same `*const Scope<'static>` re-attach site as
`module_child_scope_transmute_does_not_dangle`, exercised end-to-end through
the scheduler path the binder follows.

- `module_body_dispatch_does_not_dangle`

**`TypeExpr::builtin_cache` lifetime lift** ([src/machine/model/ast.rs](../src/machine/model/ast.rs)) — the
Layer-1 resolution cache stores `KType<'static>` because the builtin-only
`from_type_expr` path never carries arena-pinned `Module` / `Signature`
references. The cache-hit path in `ExpressionPart::resolve_for` clones the
cached value and transmutes `KType<'static> → KType<'a>` to hand it back at the
caller's lifetime. Sound because the clone is owned-data-only (no aliasing into
the cache), but tree borrows still needs to see the lift on a non-`'static` `'a`.

- `builtin_cache_lifetime_lift_does_not_dangle`

**`NodeStore::reinstall_with_frame` slot re-anchor** ([src/machine/execute/scheduler/node_store.rs](../src/machine/execute/scheduler/node_store.rs)) —
the Replace arm transmutes `frame.scope()` from its receiver-bound borrow to
`'a` (the slot-storage lifetime) before installing it in `self.nodes[id]`,
which co-locates the `Rc<CallArena>` witness with the re-anchored scope.
Exercised by the dispatch-time parking shapes that reinstall through this entry
point (and transitively by user-fn TCO; that path is covered by the MATCH-on-
`Tagged` recursion test above).

- `lift_park_minimal_program_for_miri`
- `replay_park_minimal_program_for_miri`

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
- 2026-05-28: 702.20s — 30 tests, 0 leaks, 0 UB
- 2026-05-28: 703.43s — 30 tests, 0 leaks, 0 UB
- 2026-05-25: 625.29s — 30 tests, 0 leaks, 0 UB
- 2026-05-20: 597.64s — 29 tests, 0 leaks, 0 UB
- 2026-05-18: 507.72s — 27 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
