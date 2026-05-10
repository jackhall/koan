# Uniform §7 / §8 handling for Type-tokens in value slots

**Problem.** §7 auto-wrap and §8 replay-park
([design/execution-model.md](../design/execution-model.md#dispatch-time-name-placeholders))
fire uniformly for `ExpressionPart::Identifier` in non-literal-name slots: a
bare `z` in `LET y = z` rewrites to `(z)`, dispatches through `value_lookup`,
and parks on `z`'s placeholder if the binder hasn't finalized.
[`classify_for_pick`](../src/dispatch/runtime/dispatcher.rs) treats
`ExpressionPart::Type` asymmetrically:

- *Wrap.* Type-tokens wrap only when the slot is *not* `KType::Any` or
  `KType::TypeExprRef`. `MAKESET IntOrd` (slot `KType::SignatureBound`)
  wraps and routes through the
  [`value_lookup` TypeExprRef overload](../src/dispatch/builtins/value_lookup.rs);
  `LET T = Number` (slot `KType::Any`) does *not* wrap and instead flows
  through `resolve_for(Any)` as a literal `KObject::TypeExprValue`.
- *Park.* Type-tokens never appear in `ref_name_indices`, so a non-binder
  TypeExprRef slot holding a Type-token (e.g. the `m` slot of `IntOrd :|
  OrderedSig`) skips §8 replay-park entirely. The dispatch path relies on
  FIFO queue ordering to land MODULE/SIG declarations before their
  consumers; placeholder-driven forward references for Type-tokens have no
  scheduler-level wait edge.

The asymmetry is no longer load-bearing for the *wrap* half: every builtin
type name is now bound in `Scope::data` via
[`Scope::register_type`](../src/dispatch/runtime/scope.rs), so a Type-token
in an `Any` slot would resolve through `value_lookup` to the same
`TypeExprValue` the literal path produces. Dropping the wrap asymmetry
requires the *park* half to land too — without §8 for Type-tokens, an
ascription operand like `IntOrd :| OrderedSig` whose binder is a sibling
top-level statement races and surfaces `UnboundName` instead of waiting.

The park extension exposes a scheduler bug. When (a) a multi-statement
SIG/MODULE body contains a `LET <TypeToken> = <Type-token in Any slot>`
statement that itself produces a wrap-induced sub-Dispatch (so the body
statement's slot becomes a Lift on a Bind on the sub-Dispatch), and (b) a
sibling top-level statement §8-parks on the SIG/MODULE's placeholder, the
SIG/MODULE's top-level Lift slot fails to terminalize. The Combine fires
and `bind_value` installs the binding, but the Lift's wake from the
Combine's terminal write is dropped somewhere in the
`notify_list` / `pending_deps` chain through the chained Lifts plus the
§8-park notify edge. Minimal repro:

```
SIG OrderedSig = ((LET Type = Number) (LET b = 0))
LET FirstAbstract = (OrderedSig)
```

`OrderedSig` ends up bound in `Scope::data`, but `read_result` on the SIG
slot panics with "result must be ready by the time it's read." The slate of
three failures
(`opaque_ascription_mints_distinct_module_type_per_application`,
`roadmap_example_int_ord_with_ordered_sig`, `module_type_of_resolves_via_module_member`)
all reduce to this shape.

**Impact.**

- *Single name-resolution path for Identifier and Type-token.* §7 wrap and
  §8 replay-park fire on the same rule for both bare-name kinds. The
  literal-TypeExprValue carve-out for `Type-in-Any` in `classify_for_pick`
  goes away. `LET T = Number` and `LET y = z` walk the same scheduler
  path: wrap → sub-dispatch → `value_lookup` → bound value.
- *Type-token forward references park on placeholders.* Sibling top-level
  statements that name a not-yet-finalized SIG/MODULE/struct via
  Type-token wait via the placeholder mechanism, the same way
  Identifier-named forward references already do. Submission order stops
  being load-bearing for correctness.
- *Single ascription dispatch path.* The `Module, Signature` overload at
  [`ascribe.rs`](../src/dispatch/builtins/ascribe.rs) handles every
  ascription call once Type-tokens resolve through the wrap, so the
  parallel `Type, Type` overload (and the Type-token branches of
  `resolve_module` / `resolve_signature` in
  [`module.rs`](../src/dispatch/values/module.rs)) collapse out of the
  dispatcher.
- *Smaller dispatcher surface.* `classify_for_pick`'s match collapses to
  the Identifier-vs-Type-symmetric form. `accepts_for_wrap` and
  `part_to_slot` already handle both bare-name kinds; the asymmetry is
  isolated to the slot-type carve-out.

**Directions.**

- *Diagnose the chained-Lift + §8-park notify chain — open.* The deadlock
  shape — Combine terminalizes but its outer Lift never wakes when a §8
  park is also installed on that Lift's slot — is the load-bearing unknown.
  Candidates: (a) `register_slot_deps` runs after the Replace's Lift
  rewrite and may not see the §8 park's already-installed `node_dependencies`
  edge, double-counting or short-circuiting `pending_deps`; (b)
  `notify_consumers` for the Combine's terminal write fires before the
  Lift's rewrite-time `register_slot_deps` installs the consumer in
  `notify_list[combine_id]`, leaving the Lift permanently parked; (c) the
  §8 park's `Replace` to `Dispatch(rewritten)` interacts with the
  binder-pre_run placeholder install in a way that re-installs a
  `pending_deps` count without a corresponding wake. Targeted
  reproduction-tests in [`src/execute/scheduler.rs`](../src/execute/scheduler.rs)
  pin the failing slot's state at each tick to localize the gap.
- *§8 replay-park covers Type-tokens — decided shape.* `classify_for_pick`'s
  literal-name arm pushes both Identifier and Type-token parts into
  `ref_name_indices` for non-binder picks. `run.rs`'s §8 walk extracts the
  name from either part variant. Binder slots stay un-parked (the slot is
  a *declaration*, not a reference). Implemented and reverted in this work
  pending the deadlock fix.
- *§7 wrap for `Type-in-Any` — decided.* `classify_for_pick`'s value-slot
  arm pushes Type-tokens into `wrap_indices` regardless of whether the
  slot is `Any` or a more refined type. The `register_builtin_types`
  registration in [`src/dispatch/builtins.rs`](../src/dispatch/builtins.rs)
  is the prerequisite (already shipped) — without it the wrapped
  `value_lookup` fails on `Number` etc. with `UnboundName`. Implementation
  blocked behind the deadlock diagnosis above.
- *`Type, Type` ascription overload removal — decided.* Once Type-tokens
  wrap uniformly, `IntOrd :| OrderedSig` resolves through `value_lookup`
  to `(KModule, KSignature)` futures and the `Module, Signature` overload
  fires. The `Type, Type` overloads at
  [`ascribe.rs`](../src/dispatch/builtins/ascribe.rs) and the Type-token
  branch of `resolve_module` / `resolve_signature` in
  [`module.rs`](../src/dispatch/values/module.rs) become dead code.
  Removal lands as a follow-up cleanup once the unified path is in.

## Dependencies

**Requires:**

**Unblocks:**

No hard prerequisites; no roadmap items downstream. The asymmetric carve-out
keeps the language usable in the meantime — `LET T = Number` works via the
literal path, `MAKESET IntOrd` works via the wrap path, and ascription
forward references happen to work via FIFO submission order. The
unification is a substrate cleanup, not a user-visible feature; ship when
the scheduler bug is diagnosed and a regression-pinning slate test is in
place.
