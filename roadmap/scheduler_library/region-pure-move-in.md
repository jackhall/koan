# Region-purity typed at the move-in allocs

**Problem.** The substrate's empty-reach ("resident") construction path seals
a value under the pins-nothing witness, which is sound only if the value's
borrows reach no region the seal fails to name. Two surfaces accept a
**pre-built** value and store it erased, so the `for<'b>` brand never vets
the value's internal borrows — the value's free lifetime simply unifies with
the caller's: the library's public move-in
[`RegionHandle::alloc_resident`](../../workgraph/src/witnessed/region.rs),
wrapped by koan's `RegionBrand` veneers (`alloc_ktype` / `alloc_object` in
[arena.rs](../../src/machine/core/arena.rs), reached from the bare
`alloc_type` arm of `KoanStepContextExt` in the same file), and
`RegionBrand::alloc_object_witnessed` (also arena.rs), which moves its
pre-built value through the brand-confined `Region::alloc` — the closure
receives only the already-stored reference, so the brand vets nothing about
the moved-in value either. What upholds purity is a
call-site convention — the `carrier == None ⟺ value is region-pure` match
arms in [val_decl.rs](../../src/builtins/val_decl.rs) and
[newtype_def.rs](../../src/builtins/newtype_def.rs), enforced by comment.
Choosing the bare arm for a region-borrowing value compiles clean and
under-pins: a latent dangle.

The variant split that makes a fix tractable
([ktype.rs](../../src/machine/model/types/ktype.rs) header): only the
module-family variants — `Module`, `Signature`, `AbstractType` — hold
`&'a` region pointers; **every other `KType` variant is owned** (`Rc`'d
sets, strings, kinds), and the `alloc_object_witnessed` callers pass owned
payloads too (`KString` in [print.rs](../../src/builtins/print.rs),
`KExpression` in [quote.rs](../../src/builtins/quote.rs)). So the callers
form two tiers: an owned-payload tier expressible at `'static` (a value
with any region borrow cannot be `'static`, so the compiler rejects it —
`KType`'s lifetime invariance means an owned-payload value nominally typed
`'a`, like `kt_ref.clone()`, needs a safe variant-wise rebuild to reach the
`'static` form), and a module-family tier
([sig_def.rs](../../src/builtins/sig_def.rs),
[with.rs](../../src/builtins/type_ops/with.rs), a `VAL` declared type)
whose borrows need a residence check or a carrier fold instead of an
assertion. The runtime residence primitive already exists:
`Region::owns_addr`
([region.rs](../../workgraph/src/witnessed/region.rs)).

**Acceptance criteria.**

- The lifetime-free tier is compile-enforced: the bare move-in surfaces
  accept only `'static` values (`K::At<'static>` at the library move-in,
  `KType<'static>` / `KObject<'static>` at the koan veneers), so a value
  carrying any region borrow is a compile error there.
- A safe, checked conversion (no new `unsafe`) produces the `'static` form
  by variant-wise rebuild, returning `Some` exactly when no module-family
  variant appears anywhere in the value.
- A module-family-borrowing value cannot reach an empty-reach seal through
  any public path: it is either residence-checked or carrier-folded, per the
  Tier-2 Direction below.
- The `carrier == None ⟺ region-pure` comment-convention arms in
  val_decl.rs and newtype_def.rs are gone — the obligation is discharged by
  type or by check, not by comment.
- A `compile_fail` doctest pins that a region-borrowing value is rejected at
  the `'static` move-in surface.

**Directions.**

- *Two tiers — decided.* The owned-payload tier is compile-enforced at
  `'static`; the module-family tier gets its own mechanism. One mechanism
  for both is not available: `'static` is too strong for a value that
  legitimately borrows a region the seal's context covers.
- *Tier-2 mechanism — open.* (a) A checked resident seal that verifies each
  module-family payload address against the destination region via
  `Region::owns_addr`, erroring loudly on a foreign borrow; (b) route the
  module-family tier through the witnessed path unconditionally (fold the
  binding scope's stored reach or require a carrier); (c) restructure the
  call sites so composite and payload are born inside one brand closure.
  Recommended: (a) — smallest disturbance, and it converts the latent dangle
  into a loud error.
- *Conversion failure policy — open.* When the checked `'static` rebuild
  returns `None` at a site that expected purity: (a) structured `KError`
  matching newtype_def's non-panicking style; (b) `expect` as an internal
  invariant violation. Recommended: (a).

## Dependencies

**Requires:** none.

**Unblocks:**

- [Publishing the workgraph crate](workgraph-extraction.md) — tightens the
  published `alloc_resident` surface before the API freezes.
