# Handle-headed construction operands

**Problem.** Koan discharges the `HasRegionHandle` obligation five times
([arena.rs](../../src/machine/core/arena.rs)) — one `unsafe impl` per
construction-operand shape: the bare destination-region operand
(`RegionRefFamily`, projecting `RegionBrand<'r>`) and four brand-headed
tuples (`RegionTypeFamily`'s `(RegionBrand, &KType)`, the two aggregate
builders' `(RegionBrand, Vec<Held>)` / `(RegionBrand, Vec<(String, Held)>)`,
and `RecordFieldsFamily`'s `(RegionBrand, Vec<(String, KObject)>)`,
declared in
[constructors.rs](../../src/machine/execute/dispatch/constructors.rs)).
Every impl body is the same one-liner (`self.0.handle()`), and every SAFETY
comment states the same structural claim — "a brand-headed operand re-homes
through its head" — which the library already discharges once for its own
operand shape, the `(&'b Region<P>, T)` blanket in
[step_ctx.rs](../../workgraph/src/witnessed/step_ctx.rs). The koan impls
exist only because koan's operand heads are the `RegionBrand` veneer rather
than the library's `RegionHandle`, so these are five per-embedder repeats of
a claim the library could own; they are also the only `unsafe` left in
arena.rs.

**Acceptance criteria.**

- `workgraph` discharges the handle-headed obligation once: a base
  `HasRegionHandle` impl for `RegionHandle<'b, P>` and a blanket for
  `(RegionHandle<'b, P>, T)` tuples, beside the existing
  `(&'b Region<P>, T)` blanket.
- Koan's construction-operand families project handle-headed live forms and
  carry no `HasRegionHandle` impl of their own.
- The destination-region operand rides the library's `RegionHandleFamily`;
  no koan family projects a bare `RegionBrand`.
- [arena.rs](../../src/machine/core/arena.rs) contains no `unsafe`.

**Directions.**

- *Brand ergonomics — decided.* Relocate closures that want the koan-typed
  `alloc_*` veneers rebuild the veneer locally from the operand's head
  (`RegionBrand(handle)`); the library learns nothing about the veneer.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Publishing the workgraph crate](workgraph-extraction.md) — the blanket
  impls are published surface; they land before the API freezes.
