# Eager wrap-slot resolution + scheduler duplication cleanup

Replace the dispatch-phase `apply_auto_wrap` rewrite (and its companion
`schedule_deps` sub-Dispatch path) with eager name lookup at dispatch time,
splicing the resolved value directly into the slot and installing park edges
for any still-pending placeholders. Rides on the
[`add_combine` park-producer
taxonomy](../src/machine/execute/scheduler/finish.rs) that landed with the
scheduler-reclaim fix: a slot can now park on a sibling producer without that
producer being cascade-freed, so the sub-Dispatch detour no longer earns its
keep. Folds in three related scheduler duplications surfaced while
investigating the rewrite.

**Problem.** Today's `apply_auto_wrap`
([`scheduler/dispatch.rs:416`](../src/machine/execute/scheduler/dispatch.rs))
rewrites every bare `Identifier(name)` / leaf `Type(t)` in a `wrap_indices`
slot into a single-name `Expression([…])`, which `schedule_deps`
([`dispatch.rs:272`](../src/machine/execute/scheduler/dispatch.rs)) then
schedules as a sub-Dispatch. The sub-Dispatch routes through
[`value_lookup::body_type_expr`](../src/builtins/value_lookup.rs) (Type case)
or `try_short_circuit`'s bare-Identifier path, and its resolved `&'a KObject`
gets spliced back into the slot via the Bind's `run_bind` finish. For the
common `MAKESET IntOrd`-shape call (one wrap-slot, value already bound in
scope), this means one extra sub-Dispatch slot plus a Bind slot per wrap entry
just to read a name. The same Identifier-to-sub-Expression rewrite is also
open-coded in
[`literal.rs::classify_aggregate_part`](../src/machine/execute/scheduler/literal.rs)
for dict keys/values and list elements with `wrap_identifiers` set.

Three further duplications cluster around the same code path:

- *Scope-resolve + park-on-Placeholder open-coded three ways.*
  [`try_short_circuit`](../src/machine/execute/scheduler/dispatch.rs)
  (single Identifier), `try_replay_park` (`ref_name_indices` loop), and
  `park_pending_and_redispatch` (producer list from a tentative tie) each do
  `scope.resolve(name)`, check `is_result_ready(producer)`, propagate the
  producer's error if terminal-Err, run `would_create_cycle`, then
  `add_park_edge`.
- *`schedule_deps` and `schedule_eager_fallthrough` are near-identical.* Both
  walk `expr.parts` and sub-Dispatch each `Expression` / `ListLiteral` /
  `DictLiteral`, differing only in whether to filter by an `eager_indices`
  set.
- *Five sites open-code `e.clone_for_propagation().with_frame(Frame::…)`.*
  `run_bind`, `run_combine`, `run_catch`, `park_pending_and_redispatch`, and
  `try_replay_park` each attach a labelled frame to a propagated dep error;
  only the label string varies.

**Impact.**

- *Common wrap-slot calls skip a scheduler slot per wrap entry.* `MAKESET
  IntOrd`-shape calls bind the picked function directly when the resolved
  value can be spliced eagerly — no sub-Dispatch, no Bind detour.
- *Forward-name parking flattens.* A wrap-slot waiting on a forward producer
  parks the dispatch slot itself via a `Notify` edge instead of routing
  through a sub-Dispatch's `Lift(Pending)`; one wake edge replaces a chain.
- *`wrap_indices` and `ref_name_indices` collapse to one rail.* Both ride the
  same `resolve_name_part` helper; the only branch is whether to splice the
  resolved value into the slot (wrap) or leave the bare token alone
  (ref_name).
- *Dict / list literal name slots ride the same eager path.*
  `classify_aggregate_part`'s `wrap_identifiers` arm drops its
  Identifier-to-sub-Expression rewrite and calls `resolve_name_part`
  directly.
- *Picker's `accepts_for_wrap` admission stays as-is.* The picker still needs
  to speculatively admit bare-name parts in typed slots — that's job #2 of
  auto-wrap, separate from the shape rewrite this item removes.

**Directions.**

- *Helper surface — decided.* One free function shared by every name-resolve
  site:

  ```rust
  enum NameOutcome<'a> {
      Resolved(&'a KObject<'a>),
      Parked(NodeId),
      ProducerErrored(KError),
      Unbound(String),
  }
  fn resolve_name_part<'a>(
      scope: &'a Scope<'a>,
      part: &ExpressionPart<'a>,
  ) -> NameOutcome<'a>;
  ```

  Handles bare `Identifier(name)` via `scope.resolve(name)` and bare leaf
  `Type(t)` via the hoisted `coerce_type_token_value` helper.
- *Prereq: hoist value_lookup's Type-token coercion — decided as its own PR.*
  Today's sub-Dispatch path routes bare `Type(t)` through
  `value_lookup::body_type_expr`'s coercions (TypeNameRef → KType stamping,
  module-identity surface, ascription form handling). Lift those into a free
  `coerce_type_token_value(scope, t)` and re-route the existing builtin
  overload to call it. After that, the dispatch phase can call the helper
  directly; the auto-wrap removal becomes a pure deletion rather than a
  re-implementation. PR-sized; lands before this item.
- *Dispatch rewrite — decided.* `run_dispatch` Phase 3 walks
  `resolved.slots.wrap_indices`, calls `resolve_name_part` per slot, and
  either splices `Future(obj)` into the slot or pushes the producer onto a
  `producers_to_wait` list shared with Phase 4's `ref_name_indices` walk.
  Phase 5's `wrap_indices` branch (in `schedule_deps`'s lazy-arm and
  `schedule_eager_fallthrough`) drops out.
- *Combined `producers_to_wait` install — decided.* Phase 3 and Phase 4
  share one list, one `add_park_edge` loop, one re-dispatch on wake.
  Today's two-phase park-then-redispatch becomes one. Subsumes
  `park_pending_and_redispatch`'s loop too (4 callers → 1).
- *Schedule-loop collapse — decided.* `schedule_deps` (None-arm) and
  `schedule_eager_fallthrough` collapse into one helper taking
  `Option<&[usize]>` as the eager-filter; the lazy-candidate branch passes
  `Some`, the Deferred-fallthrough passes `None`. Same PR as the wrap
  removal, since both touch `schedule_deps`.
- *`literal.rs::classify_aggregate_part` migration — decided.* Once
  `resolve_name_part` exists, `wrap_identifiers`-branch entries call it
  directly to eager-resolve dict keys/values and list elements. Removes the
  last open-coded copy of the Identifier-to-sub-Expression rewrite.
- *Error-propagation helper — decided.* `propagate_dep_error(e, frame_label)
  -> KError` centralizes the five `clone_for_propagation().with_frame(…)`
  sites; falls out for free during #1's consolidation. `run_catch`'s
  frameless variant passes an empty label and skips the frame attach.
- *`val_decl.rs:38` defensive comment — deferred.* The "avoid
  `elaborate_type_expr` because Placeholder NodeIds install OWNED edges"
  warning is stale post the add_combine park-producer fix. Replacing the
  sub-Dispatch detour with a direct `elaborate_type_expr` call (feeding any
  returned Placeholder as a `park_producer` to `add_combine`) is independent
  cleanup; doesn't block this work.
- *Test surface — decided.* Equivalence tests for the new
  `coerce_type_token_value` helper (pins every coercion the current
  sub-Dispatch path produces: KTypeValue stamping, TypeNameRef
  materialization, module-identity, ascription forms). Integration cases at
  `tests/eager_wrap_resolve.rs` for `MAKESET IntOrd`, forward-Identifier
  wrap parking, `LIST_OF Mo.Ty` (Deferred path — unchanged), and
  `MAKESET IntOrd :| OrderedSig` (parens-Expression wrap-slot — eager
  sub-Dispatch still applies).
- *Risk surface — decided.* The coercion-lift prereq is load-bearing. A
  missed coercion means some downstream `bind` slot quietly receives a
  less-coerced carrier and a typed slot's `matches` check fails. Mitigation:
  the prereq lands as its own PR with the equivalence tests passing before
  any dispatch-phase plumbing changes.

## Dependencies

**Requires:**

**Unblocks:**

The detailed pre-PR sequencing notes (helper signatures, splice mechanics,
test cases) live in [`scratch/auto-wrap-eager-resolve.md`](../scratch/auto-wrap-eager-resolve.md) and migrate
here when the work is staged.
