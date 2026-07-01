# Witnessed type and region operands

The capstone: the last asserted operands become computed carriers, and `Witnessed::new` — with no
remaining caller — is deleted.

**Problem.** The construction operands pair a destination region / brand with a foreign `&KType`
identity via an asserted `Witnessed::new`, stating in prose that the identity's region is pinned by the
dest frame's `outer` chain. Six sites across three carrier families: `RegionTypeFamily` in the newtype /
tagged-union constructors ([`dispatch/constructors.rs`](../../src/machine/execute/dispatch/constructors.rs),
three sites) and the `CATCH` `Result` build ([`catch.rs`](../../src/builtins/catch.rs)),
`ContractHomeFamily` in the declared-return contract home
([`finalize.rs`](../../src/machine/execute/finalize.rs)), and `RegionRefFamily` in the relocate
destination region ([`runtime.rs`](../../src/machine/execute/runtime.rs)). Each asserts co-location the
constructor cannot check. [`Witnessed::new`](../../src/witnessed.rs) — the asserted-co-location
constructor — survives only to back these operands plus the object read and the bare-`Done` terminal;
once those and these are retired, it has no caller.

**Acceptance criteria.**

- The `RegionTypeFamily` / `ContractHomeFamily` operand is assembled by `yoke`ing the destination brand
  into its single region, lifting with `into_set`, and `merge`ing a delivered type-identity carrier; the
  `RegionRefFamily` destination likewise rides a witnessed carrier — so the nominal identity crosses the
  build brand witnessed by its own region, not asserted. This reuses the type channel's `seal_type` /
  `seal_module` delivery of
  [§Storage and access](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into).
- The operand `Witnessed::new` sites — `constructors.rs` (three), `catch.rs`, `finalize.rs`
  (`ContractHomeFamily`), and `runtime.rs` (`RegionRefFamily`) — no longer exist.
- `Witnessed::new` does not exist. No carrier is built by pairing an already-built value with an
  independently-supplied witness anywhere in the workload — the transitional constructor
  [§Construction](../../design/per-node-memory.md#construction-yoke-merge-map-and-one-wrapper-per-node)
  describes as keeping no blessed home is removed.

**Directions.**

- *Type-identity carrier delivery — decided.* The nominal identity in `CtorKind` (and `catch`'s `Result`
  build, and the declared-return contract home) rides a delivered type carrier so the operand is
  `merge`d, not asserted; reuses the type channel's existing `seal_type` delivery. The `yoke` of the
  destination brand goes through the foundation's `yoke_branded` + `into_set`, then `merge`s the identity
  carrier.
- *`Witnessed::new` deletion — decided.* Owned here as the capstone: once the object read, the
  bare-`Done` terminal, and these operands are retired, delete `Witnessed::new`.

## Dependencies

The operand retyping needs only the foundation; the final `Witnessed::new` deletion additionally needs
the object-read and bare-`Done` items to have retired their callers.

**Requires:**

- [The honest single-region witness substrate](../../src/witnessed.rs) — the operand `yoke` + `into_set` + `merge` builds on the honest witness surface.
- [Object read-site carrier](object-read-carrier.md) — its object-read `Witnessed::new` callers must be gone before the deletion.
- [Bare-`Done` terminal collapse](bare-done-collapse.md) — its bare-`Done` `Witnessed::new` caller must be gone before the deletion.

**Unblocks:** none.
