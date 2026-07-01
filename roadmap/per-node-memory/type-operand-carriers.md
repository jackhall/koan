# Witnessed type and region operands

The capstone: the last asserted operands become computed carriers, and `Witnessed::new` — with no
remaining caller — is deleted.

**Problem.** The construction operands pair a destination region / brand with a foreign `&KType`
identity via an asserted `Witnessed::new`, stating in prose that the identity's region is pinned by the
dest frame's `outer` chain. Seven sites across three carrier families: `RegionTypeFamily` in the newtype /
tagged-union constructors ([`dispatch/constructors.rs`](../../src/machine/execute/dispatch/constructors.rs),
three sites) and the `CATCH` `Result` build ([`catch.rs`](../../src/builtins/catch.rs)),
`ContractHomeFamily` in the declared-return contract home
([`finalize.rs`](../../src/machine/execute/finalize.rs)), and `RegionRefFamily` in the relocate
destination region — both the storage-bound relocate ([`runtime.rs`](../../src/machine/execute/runtime.rs))
and the literal-consumer pull ([`single_poll.rs`](../../src/machine/execute/dispatch/single_poll.rs)). Each
asserts co-location the constructor cannot check. [`Witnessed::new`](../../src/witnessed.rs) — the
asserted-co-location constructor — survives only to back these operands; the object-read and bare-`Done`
callers are already retired, so once these are, it has no caller.

**Acceptance criteria.**

- The newtype / tagged-union / `CATCH` construction operand `merge`s a delivered type-identity carrier
  whose witness names the identity's own reach — sourced from the binding's stored per-binding type reach
  ([`Bindings.types`](../../src/machine/core/bindings.rs)), never re-derived by walking the `SetRef` — so
  the nominal identity crosses the build brand witnessed by its own region, `merge`d rather than asserted.
- The declared-return contract-home operand ([`finalize.rs`](../../src/machine/execute/finalize.rs)) is
  born region-pure in the contract's home region and folded by [`merge`](../../src/witnessed.rs) under the
  producer's own witness — which pins the home ancestor via its `outer` chain — with no witness supplied by
  assertion and no change to the `Copy` `ReturnContract`.
- The `RegionRefFamily` relocate destination rides a witnessed carrier: `yoke`d into its owning frame
  where one owns it, or born under the empty set where the destination region is externally pinned (the
  drained run root).
- The seven operand `Witnessed::new` sites no longer exist.
- `Witnessed::new` does not exist; no production site pairs an already-built value with a
  separately-asserted witness.

**Directions.**

- *Type-identity carrier delivery — decided.* The construction identity rides a delivered type carrier
  whose witness carries the identity's reach (from [`Bindings.types`](../../src/machine/core/bindings.rs)),
  `merge`d into the operand. Chosen over re-minting the identity region-pure under the dest frame: that is
  sound only while `RecursiveSet` is heap-`Rc`'d (identity reach empty), and rots the day `RecursiveSet`
  becomes region-allocated — the identity would then reach its set's region, which the dest frame need not
  pin. The delivered carrier folds that reach automatically (empty today, the set's region after the
  migration) with no site change, so this item is also the groundwork that de-risks that future migration.
- *Construction-identity threading — decided.* Thread the identity's stored reach from its binding
  through `CtorKind` and build the operand's identity carrier via
  [`resident_type_carrier`](../../src/machine/core/scope.rs); do **not** stage a hardcoded-empty-reach
  local build. The construction identity resolves through the *lexical* chain, but a per-call frame's
  *storage* `outer` chain omits lexical ancestors under TCO — so the value carrier's own witness need not
  pin the identity's region once `RecursiveSet` is region-allocated, making a born-empty operand a latent
  TCO-dependent use-after-free. Naming the reach from the binding is the only sound source. Only the
  `Constructor` dispatch arm reaches the construction operand, so the reach threads without touching the
  functor / placeholder branches that share `resolve_type_with_chain`.
- *Contract-home operand — decided.* Born region-pure in the home region via
  [`resident`](../../src/witnessed.rs) and folded by `merge` under the producer's witness; the declared
  type is always a producer lexical/outer ancestor, so no reach threads through `ReturnContract` and its
  `Copy` is preserved.
- *`RegionRefFamily` destination — decided.* `yoke_branded` where a frame owns the destination; a confined
  empty-set `resident` where the destination outlives the carrier (the drained run root).
- *`Witnessed::new` deletion — decided.* Owned here as the capstone: once these operands are retyped — the
  object-read and bare-`Done` callers already gone — delete `Witnessed::new`.

## Dependencies

The object-read item has shipped, so the `Witnessed::new` deletion is unblocked. The construction-identity
carrier consumes the per-binding type reach that item stored on `Bindings.types`. A future region-allocated
`RecursiveSet` builds on the delivered-carrier reach landed here; it is not yet filed as a roadmap item.

**Requires:**

- [The honest single-region witness substrate](../../src/witnessed.rs) — the operand `yoke` + `into_set` + `merge` builds on the honest witness surface.
- [Per-binding type reach](../../src/machine/core/bindings.rs) — the delivered identity carrier's witness is the reach stored on `Bindings.types`.

**Unblocks:** none.
