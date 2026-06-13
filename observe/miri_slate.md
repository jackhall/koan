# Miri audit slate

<!-- slate-fingerprint
src/machine/core/arena.rs: 16
src/machine/core/kfunction/body.rs: 3
src/machine/core/scope_ptr.rs: 6
src/machine/execute/scheduler/execute.rs: 1
src/machine/model/values/module.rs: 1
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

22 tests, grouped by the unsafe site each pins down. Names below are the exact
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

**`ErasedContract` re-attach** ([src/machine/core/kfunction/body.rs](../src/machine/core/kfunction/body.rs))
— the contract-lifetime erasure that mirrors `ScopePtr` for `ReturnContract`: `erase` forgets the
lifetime for storage on a node's lifetime-free `Frame`, and the `unsafe` `reattach` transmutes
`ReturnContract<'static>` back to a lifetime witnessed by the cart `Rc` that pins the contract's
home arena (the cart's frame-outer arena — a strict ancestor). The unbounded re-attach call site
in [src/machine/execute/scheduler/execute.rs](../src/machine/execute/scheduler/execute.rs) (the
Done-boundary return-type check) runs the same transmute; end-to-end, `recursive_tagged_match_no_uaf`
exercises it through a MATCH arm's `-> :T` carried across tail recursion. This test pins the
erase → reattach round-trip directly.

- `erased_contract_reattach_roundtrip`

**`ErasedContract` re-attach — Done-boundary call site** ([src/machine/execute/scheduler/execute.rs](../src/machine/execute/scheduler/execute.rs))
— the `unsafe { contract.reattach(&cart) }` in the Done arm runs the transmute defined in the
group above; it carries no transmute of its own, so the same `erased_contract_reattach_roundtrip`
(and end-to-end `recursive_tagged_match_no_uaf`) pins it. No separate minimal test.

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
- 2026-06-13: 532s — 22 tests, 0 leaks, 0 UB
- 2026-06-13: 821s — 22 tests, 0 leaks, 0 UB
- 2026-06-13: 873s — 22 tests, 0 leaks, 0 UB
- 2026-06-13: 538s — 22 tests, 0 leaks, 0 UB
- 2026-06-13: 543s — 22 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->
