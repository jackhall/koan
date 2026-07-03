# Miri audit slate — workgraph

The canonical list of tests Miri's tree-borrows mode signs off on for the
`workgraph` crate's memory safety — the witnessed carrier substrate and the
generic region engine. Each test is a minimal-shape mirror of an unsafe site
in the crate; the slate passes when Miri reports zero process-exit leaks and
zero UB across the whole list.

Sibling to [koan's own slate](../../observe/miri_slate.md) — split because
these tests live in the `workgraph` crate's own lib test binary, a separate
`cargo test` target from koan's. Not wired into `tools/observe_tests.py`'s
automated drift check (that stays scoped to koan's own `src/`): this is plain
documentation, kept current by hand, for a manual run per
[.claude/skills/miri/SKILL.md](../../.claude/skills/miri/SKILL.md). Memory-model
invariants the slate verifies live in
[design/memory-model.md](../../design/memory-model.md).

## The slate

14 tests, grouped by the unsafe site each pins down. Names below are the
exact test identifiers; pass them after `--` in the Miri command.

**`retype` primitive — `Erased<T>` / `Witnessed<T, W>`** ([src/witnessed.rs](../src/witnessed.rs))
— the single audited lifetime-retype every carrier family routes: `retype<A, B>` (a
`transmute_copy` behind a `ManuallyDrop`, the one site `transmute`'s GAT size-proof can't cover),
reached through `Erased<T>::erase` / `reattach`, the
consuming externally-witnessed `SealedExtern::open` (which reattaches a witness-less carrier — or a
`zip`-combined product / `seal_option` optional of carriers — at a generative `for<'b>` brand the
supplied witness pins), and through the `Witnessed` accessors: the rank-2 branded `with`
(borrow + read) and `map` (consume + transform), the borrow-bounded `read` that hands the carrier
*out* at the `&self` borrow — sound because its content lifetime is the borrow itself (not a free
`'b`), so the bundled `Witness` pins it for exactly that long — and the rank-2 branded `merge`, which
re-anchors *two* carriers under one `'b`, runs a binding projection, and re-seals under the
descendant witness (the one whose ancestor-chain pin keeps both regions live), rejecting unrelated
carts. The co-location-enforcing constructor `yoke` sources its carrier from the witness's region
through a `for<'b>` closure (no `unsafe` of its own — it routes the safe `erase`), so it is exercised
for the brand discipline, not a retype. The `unsafe impl Reattachable` families declare
layout-invariance and carry no runtime `unsafe` of their own — they are exercised through this
primitive using a generic `Rc<TestCart>` stand-in (this crate names no embedder family, so the
stand-in mirrors an embedder's own `Reattachable` families structurally: a covariant `&'r u32`, an
invariant `Cell<&'r u32>`, a `Box`-boxed non-`Copy` continuation, and the generic `And` product /
`OptionOf` optional families the `zip` / `seal_option` combinators seal). The tests erase a
borrow-carrying family to the `'static` store and
re-anchor it through every entry point — the witness-less helpers, the borrow-bounded `read` (read
after the original binding drops), and the `Witnessed` accessors that drop the *original* binding and
read back only through the bundled witness (the load-bearing case for the invariant `Cell<&'r u32>`
carrier) — plus `map`'s branded projection (binding a cart-coherent `&'b` value into the invariant
scope slot, the write `with` rejects). `yoke` sources a carrier from a stand-in cart's region, and
`merge` binds an ancestor-cart ref into a descendant-cart scope at the shared brand and re-seals under
the descendant (read back after both call handles drop), plus a `None`-on-unrelated-carts check.
`SealedExtern::open` is exercised distinctly from the bundled `with` / `read`: a witness-less carrier
opened against a *separately-held* `Rc` witness (invariant `Cell<&'r u32>` read back after the
original drops), a **non-`Copy`** `Box<&'r u32>` consumed by the open (the boxed-continuation shape
`Copy`-bounded `Sealed::open` excludes), and a heterogeneous `zip` of a boxed carrier + a present
`seal_option` optional + a reference opened together at one brand (plus the `None`-optional arm). The
escape-can't-compile guards are `compile_fail` doctests on `with` / `map` / `yoke` / `merge` /
`SealedExtern::open`.

An embedder's realisation of the `unsafe trait` impls this primitive routes for — Koan's
`Witness` / `WitnessRegion` / `MergeWitness` for `FrameSet`, the unified region-owner witness in
`machine/core/arena.rs` — is covered cross-crate: its region-plus-`outer`-ancestry shape is exactly
what the `Rc<TestCart>` stand-in mirrors, so `yoke_sources_carrier_from_witness_region` and
`merge_binds_ancestor_ref_into_descendant_scope` pin its yoke / merge / subsumption
(drop-an-ancestor-still-pinned-by-the-chain) UB shapes, and `merge_rejects_unrelated_carts` the
no-common-pin verdict. Koan's `FrameSet::merge` antichain logic (union with `outer`-chain
subsumption) is pinned separately by that embedder's own `frameset_*` /
`pins_region_walks_outer_chain` unit tests, which run under plain `cargo test` (no `unsafe` of their
own — the `unsafe` they exercise is this primitive).

- `erased_roundtrip`
- `read_borrow_bounded_witness_only`
- `branded_ref_reads_erased_store`
- `covariant_roundtrip_witness_only`
- `invariant_roundtrip_witness_only`
- `continuation_binds_cart_coherent_value_via_map`
- `invariant_same_brand_mutation`
- `yoke_sources_carrier_from_witness_region`
- `merge_binds_ancestor_ref_into_descendant_scope`
- `merge_rejects_unrelated_carts`
- `sealed_extern_open_externally_witnessed`
- `sealed_extern_open_consumes_non_copy`
- `sealed_extern_zip_opens_heterogeneous_at_one_brand`
- `seal_option_none_opens_to_none`

**`ReturnContract` re-attach — Done-boundary open** ([src/witnessed.rs](../src/witnessed.rs))
— an embedder's return-contract opens at its run-loop step brand alongside the continuation (a
`seal_option` optional operand of the step's `SealedExtern::open`), so it is live at the Done arm
with no reattach of its own; the `unsafe` lives in `SealedExtern::open` (`Erased::reattach`).
`erased_roundtrip` / `sealed_extern_zip_opens_heterogeneous_at_one_brand` above pin it end-to-end
(Koan's `recursive_tagged_match_no_uaf`, in that embedder's own slate, exercises the production
shape). No separate minimal test here.

**`SealedExtern::open` — run-loop step-tail open** ([src/witnessed.rs](../src/witnessed.rs))
— the `unsafe { self.value.reattach() }` inside `SealedExtern::open` runs the transmute defined in the
`retype` group above with none of its own. An embedder's run-loop routes its step's continuation,
contract, and consumer-`dest` region together through this one call at a single generative `for<'b>`
brand its start cart pins. The `sealed_extern_*` minimal tests above pin it directly; an embedder's
own scheduler-driving tests exercise it end-to-end. No separate minimal test here.

**Doctest fixture markers** ([src/witnessed/doctest_fixture.rs](../src/witnessed/doctest_fixture.rs))
— the `unsafe impl Reattachable` for `RefFamily` / `InvFamily` and `unsafe impl Witness` /
`WitnessRegion` / `MergeWitness` for `Cart` back the six `compile_fail` soundness guards and their
compiling twins (`cargo test --doc`), so a signature change to those traits has one shared
fixture to update instead of five pasted copies. Each impl is a marker with no runtime `unsafe`
operation of its own, asserting the identical `&'r u32` / `Cell<&'r u32>` layout-invariance and
owned-`Vec` fixed-address pin shapes the `retype` group's separate `Rc<TestCart>` stand-in (in
`witnessed/tests.rs`, excluded from the audit as test scaffolding) already Miri-verifies. Doctests run
under `cargo test --doc`, not Miri, so there is no separate slate test here — the shape is pinned by
the `retype` group above.

## Adding tests to the slate

Add a test to the slate when a new unsafe site lands — a transmute, raw-pointer
round-trip, interior-mutation pattern under a live shared borrow, or a cycle
shape that storage-side reasoning can't rule out. Tests are minimal-shape
mirrors of the unsafe operation, not end-to-end feature tests; they fail when
Miri reports UB or a leak, not on values.

When you add or remove a slate test, update the list above and re-run the
slate to confirm the line count matches.
