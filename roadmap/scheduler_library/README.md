# Scheduler library

The extraction of Koan's runtime substrate — scheduler, region memory, witnessed
carriers — into one Koan-agnostic library whose public surface is memory-safe by
construction. North star: [design/scheduler-library.md](../../design/scheduler-library.md).
This project's items land the consumer API and the boundary moves incrementally;
each item is sized for one PR. Objectives not yet scoped as items are listed
under "Not yet planned" below.

## Not yet planned

Objectives from [design/scheduler-library.md](../../design/scheduler-library.md)
not yet scoped as roadmap items — listed here so the project's full arc is
visible; each becomes one or more items when its prerequisites are close.

- **Regions wholesale; `Scope` as a naming layer.** Scope storage allocates
  through library region handles held by `CallFrame`; the at-will (non-step)
  allocation surface re-homes onto the library with no koan-side arena
  ownership left.
- **Further protocol combinators.** The await-all-body-slots-then-seal shape
  shared by `sig_def.rs`, `module_def.rs`, and `recursive_types.rs` becomes a
  named combinator, following the pattern
  [the resolve-or-await combinator](../../src/builtins/resolve_or_await.rs)
  sets.
- **Ambient bracket hygiene.** Closure-scoped or Drop-backed brackets for
  the per-step ambient state (`SlotStepGuard`, `swap_active_frame`,
  `active_in_contract_chain`) — no Drop backing, raw field writes.
- **Structural `Scope::seal_value` embed.** `Scope::seal_value`'s `embedded:
  Option<&Sealed<…>>` operand is still optional — its value-copy callers
  (`attr.rs` field reads, `record_projection.rs`) pass `None` for a region-pure
  source. Making a projected value's embedded reach structural — named by
  construction like the born-pure sites the step construction context landed,
  rather than by an asserted-or-absent operand — retires the `Option` on the
  last seal surface that still carries it.
- **Actual extraction.** The `workgraph` sub-crate boundary is in place
  ([design/scheduler-library.md](../../design/scheduler-library.md)); once its
  API stabilizes: docs and publishing the substrate for use outside this repo.

## Review findings — carrier-carrying-spliced-parts branch

Findings from the branch review of `master...HEAD` (the step-construction-context
and carrier-carrying-spliced-parts items), verified against the working tree.
Each entry names the site, how to recognize the defect, and the fix. Delete an
entry when its fix lands; delete the section when empty. The splice-site
correctness finding shipped as total-carrier-resolution (deleted from
`roadmap/` once its acceptance criteria were met).

1. **Per-call witness duplication and double reach fold in `invoke`.**
   [exec.rs](../../src/machine/execute/dispatch/exec.rs): `carriers_from_expr`
   (~209) calls `cell.duplicate()` per spliced arg (a `FrameSet` `Vec` clone +
   `Rc` bumps) though the duplicates are consumed only through `.witness()`
   borrows; and for user fns the reach is folded twice into the same scope
   (`extract_carried_args`'s `adopt_sealed` folds it, then the
   `frame.with_scope(... fold_reach ...)` loop folds the identical witnesses
   again). **Fix:** borrow `cell.witness()` off the working expression
   (it outlives the call) instead of duplicating, and drop the redundant
   second fold.

2. **Historical-narrative comments + a dead import kept for a doc link.**
   [lift.rs](../../src/machine/execute/lift.rs) module doc is written as a
   change log ("now lives in `machine::core`", callers "keep their path") and
   keeps `#[allow(unused_imports)] use crate::machine::core::RegionBrand`
   alive solely so a doc link resolves;
   [ast.rs](../../src/machine/model/ast.rs) ~539 explains `_marker`'s
   invariance by reference to the deleted borrowing `KExpression` ("the
   variance a borrowing `KExpression` carried"). **Fix:** rewrite both
   present-tense (state the invariant, not the history), use a
   fully-qualified doc link in lift.rs and delete the allow+import — the file
   reduces to one re-export plus `mod tests`.

3. **Duplicated test helper.**
   [ktype_predicates/tests.rs](../../src/machine/model/types/ktype_predicates/tests.rs)
   ~14's `spliced` is a byte-for-byte copy (doc comment included) of
   `test_support::spliced_part`
   ([test_support.rs](../../src/builtins/test_support.rs) ~188), and the
   module already imports from `test_support`. **Fix:** delete the local copy
   and import `spliced_part`.

4. **Two inline comments exceed the 3–4 line cap** (Claude.md:
   "Inline comments: keep these to 3-4 lines"):
   [attr.rs](../../src/builtins/attr.rs) ~330 (9 lines),
   [ctx.rs](../../src/machine/execute/dispatch/ctx.rs) ~266
   (6 lines). **Fix:** move the rationale to the file-top comment or
   [design/scheduler-library.md](../../design/scheduler-library.md) and link.

5. **The scope-to-`FinishCtx` recipe exists in three places.**
   [resolve_or_await.rs](../../src/builtins/resolve_or_await.rs) ~84
   hand-builds `FinishCtx { scope, ctx: StepContext::new(scope_frame(scope)) }`
   though `BodyCtx::finish_ctx()`
   ([action.rs](../../src/machine/core/kfunction/action.rs) ~215) produces
   the shape and the sole caller holds a `BodyCtx`;
   [union.rs](../../src/builtins/union.rs) ~251's test repeats it.
   **Fix:** thread the caller's `ctx.finish_ctx()` in, or add a
   `FinishCtx::for_scope` constructor and use it at all three sites.

## Next items

This project's items with no unshipped prerequisite — ready to start.
Regenerated by `python3 tools/doclinks.py sync-next`; do not edit by hand.


