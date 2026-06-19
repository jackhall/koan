# Fold `Dep` into `DepRequest`

`Dep` and `DepRequest` carry an identical `Dispatch { expr, placement }` / `Existing`
core — and already share `DepPlacement` — yet are spelled as two unrelated enums.

**Problem.** Two dependency-request enums describe the deps a step declares, with an
identical core:

- [`Dep`](../../src/machine/core/kfunction/action.rs)
  `{ Dispatch { expr: KExpression, placement: DepPlacement }, Existing(NodeId) }` —
  the dep currency a builtin's `Action` declares (dep-finish / Tail dependencies), in
  the core layer.
- [`DepRequest`](../../src/machine/execute/dispatch.rs)
  `{ Dispatch { expr, placement }, ListLit, DictLit, RecordLit, BodyBlock, Existing }`
  — the dep currency a dispatch `ParkThenContinue` declares, in the execute layer.

`Dep`'s two arms are byte-identical to `DepRequest`'s `Dispatch` and `Existing` arms —
same field types, and both reference the *same*
[`DepPlacement`](../../src/machine/core/kfunction/action.rs) type. `DepRequest` is a
strict superset: it adds the four aggregate-literal / body-block lowering arms. The
two are kept in lockstep by the paired
[`defer_field_list` / `defer_field_list_action`](../../src/machine/execute/dispatch/field_list.rs)
helpers.

The documented rationale for `DepRequest` living on the dispatch side is that it
"names AST … keeping `outcome.rs` AST-free." But `Dep` *also* names AST (its
`Dispatch` arm carries a `KExpression`), so the split is by layer (builtin-`Action`
currency vs dispatcher currency), not by AST-freeness — leaving the shared
`Dispatch`/`Existing` core written twice.

**Acceptance criteria.**

- The `Dispatch { expr, placement }` + `Existing(NodeId)` core is defined once; `Dep`
  and `DepRequest` become one type or both embed the shared core rather than
  re-spelling its arms.
- `DepRequest`'s four extra arms (`ListLit` / `DictLit` / `RecordLit` / `BodyBlock`)
  stay expressible without the builtin-`Action` layer naming them.
- `defer_field_list` and `defer_field_list_action` no longer maintain two parallel
  arm-for-arm constructions of the shared core.
- If the two stay distinct types, their names make the subset relationship visible —
  today `Dep` and `DepRequest` read as unrelated.

**Directions.**

- *Shared core vs accept-the-split-and-rename — open.* Either (a) extract the common
  `{ Dispatch, Existing }` into one core enum that both embed (e.g.
  `DepRequest::Simple(Dep)` plus the four lowering arms), or (b) keep them separate
  but rename so the subset relationship is legible. Recommended: (a) — they already
  share `DepPlacement`, so a shared core removes the lockstep without a layering
  change.
- *Keep the layer boundary — decided.* Whatever the shape, the builtin-`Action`
  currency stays in core (`action.rs`) and the AST-naming lowering arms stay on the
  dispatch side; the fold shares a core type, it does not move AST into `outcome.rs`.

## Dependencies

An engine-internal dispatch-path hygiene item; update
[design/execution/README.md](../../design/execution/README.md) (the dispatcher /
scheduler boundary) if the dep-request vocabulary it names changes.

**Requires:** none — engine-internal.

**Unblocks:** none tracked yet.
