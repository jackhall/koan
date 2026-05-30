# Relocate struct + tagged-union constructors to `dispatch::constructors`

Move `struct_value` and `tagged_union` out of `builtins/` to a new
`execute::dispatch::constructors` peer module, drop their registered
`struct_construct` / `tagged_union_construct` primitives, and route
`apply` directly through `construct` via an eager-subs track on
`CtorState` instead of a `BodyResult::Tail` re-dispatch.

**Problem.** Two of the construction primitives are dispatch-shape
implementations misfiled as user-facing builtins. `struct_construct`
([`src/builtins/struct_value.rs`](../../src/builtins/struct_value.rs))
and `tagged_union_construct`
([`src/builtins/tagged_union.rs`](../../src/builtins/tagged_union.rs))
are registered with `register_builtin`, but the `ConstructorCall` fast
lane in
[`single_poll.rs`](../../src/machine/execute/dispatch/single_poll.rs)
calls `struct_value::apply` / `tagged_union::apply` directly without
any name lookup. The registered names are unreachable by user code —
the bucket-lookup hit only happens because `apply` synthesizes a
`BodyResult::Tail` that re-dispatches a `[Future(schema), …]`
expression, which round-trips through Keyworded dispatch + bucket
match + the registered primitive body just to call `construct`.
[`builtins.rs:38`](../../src/builtins.rs)'s `dispatch_constructor`
helper exists solely to bridge this routing.

**Impact.**

- Constructor implementations live next to their sole caller. The
  `ConstructorCall` fast lane no longer reaches across the crate into
  `builtins/`.
- `apply` calls `construct` directly via a stateful `CtorState` that
  parks on eager subs (for nested arg expressions like
  `Point (x: a + b, y: 4)`), eliminating the tail re-dispatch through
  the Keyworded path. Constructor dispatch drops the bucket-lookup
  round-trip plus the `BodyResult::Tail` → `Dispatch` → bucket-match
  → `primitive_body` → `construct` chain.
- Builtins module count drops by two; the bridge helper
  `dispatch_constructor` retires alongside.

**Scoring.** Measured against the post-Plan-A baseline
(crate 306.71, machine::execute ~120.7 per root-loc, γ=50.0, T=325) on
2026-05-29. Re-score with `tools/modgraph.py --regenerate --baseline
observe/complexity.txt` after the working-tree edits land — the
expectation is a small crate-Δ win (two deleted builtins entries net
out two new `dispatch::constructors::*` entries; the dispatch subtree
gains one child but the cross-crate `builtins → dispatch` edge
disappears).

**Directions.**

- **Constructors land at `dispatch::constructors` peer, not merged
  into `dispatch.rs` — decided.** Earlier scoring at the pre-facade
  baseline showed the peer variant beating the merged variant
  (crate Δ −9.49 vs −8.84). Re-measure but the relative ordering
  should hold.
- **Drop the `struct_construct` / `tagged_union_construct`
  registrations and route `apply` → `construct` directly — decided.**
  `CtorState` gains an `Option<EagerSubsTrack>` modelled on
  `FnValueState::eager_subs`; the resume gathers the resolved values
  and calls `construct` synchronously.
- **`dispatch_constructor` bridge helper in `builtins.rs` —
  decided.** Deletes alongside the relocation; its only caller (the
  `ConstructorCall` fast lane) becomes self-contained.
- **Per-constructor file granularity — open.** Either one file per
  constructor (`constructors/struct_value.rs` + `constructors/tagged_union.rs`,
  preserves the existing `mod tests;` layout) *or* one merged
  `constructors.rs` (smaller cross-file surface, larger leaf). The
  per-constructor split is the recommended default; the merge becomes
  preferable only if the eager-subs walk turns out to share enough
  code between the two cases to justify a single file.

## Dependencies

**Requires:** none. Both target files (`struct_value.rs`,
`tagged_union.rs`) are self-contained at the module level and only
reach into stable substrates (arena, scope, ktype, dispatch state).

**Unblocks:** none.
