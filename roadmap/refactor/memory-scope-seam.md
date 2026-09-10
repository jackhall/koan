# Seam `Scope` between `memory` and `core`

`memory` imports nothing from the rest of Koan, and `Scope` is a lexical record holding a
`memory`-owned residence rather than a lexical record with region plumbing stapled on.

**Problem.** [`memory/frame.rs`](../../src/memory/frame.rs) names `Scope`, `ScopeId` and
`ScopeRefFamily` — the one back-edge `memory` keeps, per the module doc in
[`memory.rs`](../../src/memory.rs) — and uses them for exactly three things: the envelope's
`Delivered<ScopeRefFamily>` field type, the `with_scope` closure signature, and `scope_id`. The
module's own rule says a name that binds a payload belongs with the payload, and this field is the
one place the rule is not followed. On the other side, [`Scope`](../../src/machine/core/scope.rs)
holds a `brand: RegionBrand<'a>` and derives `region`, `brand`, `region_owner`, `frame`,
`parent_frame_pin`, `open_frame` and `adopt_as_run_frame` from it — residence and pin policy,
including the eternal-tier rule and the TCO pin-chain argument, living on a lexical type.
`ScopeId` is position-independent identity so a relocated or freed scope cannot break dispatch,
which is a memory-model property, yet it lives in `core` and is the whole of what
`model/types/{node,type_digest,recursive_group_window}.rs` import from there.

**Acceptance criteria.**

- `memory` has **no** `use crate::machine::…` outside doc comments and `#[cfg(test)]`. The
  frame shell is generic over the reattachable family it carries (`Frame<F>`); `core` declares
  `type CallFrame = Frame<ScopeRefFamily>` beside `Scope` and adds the `scope_id` read as an
  inherent impl on that instantiation. Every current call site keeps spelling `CallFrame`.
- `ScopeId` lives in `memory` (its own file, re-exported at `memory::ScopeId`), with the
  session/counter mint stated there as an identity source, not a lookup registry. The
  `model::types` imports of `ScopeId` point at `memory`.
- The **residence derivations** `Scope` makes today — `region_owner`, `frame` (with its upgrade
  invariant), and `parent_frame_pin` with the eternal-tier rule — are inherent methods on
  `RegionBrand` in `memory`; there is no wrapper type, since a wrapper over the brand would carry
  no invariant the brand lacks. `Scope` delegates to its brand; no
  `Rc<FrameStorage>`/`Weak<FrameStorage>` is upgraded from `core` or `execute`.
- The body of `Scope::open_frame` — `RegionHost::fresh` over the derived pin, `bump_born_with`
  at the generative brand, `deliver_resident`, `around` — is a `memory` door generic over the
  family, taking the child constructor as a closure. `Scope::open_frame` and
  `adopt_as_run_frame` are thin callers passing `child_for_frame_witnessed`. The pin-chain prose
  (why chaining the captured scope's owner retains exactly the frames a tail loop reaches) moves
  with the door.
- The harness's `NodeScope` classification asks the frame whether it hosts a scope's residence
  through one `memory`-side query; `scopes_eq` stays the lexical half.
- The placement rule is recorded in the `memory` module doc: `memory` owns payload-generic
  **storage shapes** (`BumpBackedMap`, and the slot array
  [slot-shaped-per-call-scopes.md](../reduce_allocs/slot-shaped-per-call-scopes.md) adds);
  `core::bindings` stays the Koan-vocabulary façade that instantiates them. `Bindings`,
  `BindingIndex`, the claim store and the write gate stay in `core`.
- Behavior is unchanged: the full slate passes, `tools/seam_equivalence.sh` passes, and the
  Miri slate is clean.

**Directions.**

- *`Bindings` in `memory` — decided, stays in `core`.* Its five tables key on `model::labels` vocabulary and
  hold `KType`, `SealedValue`, dispatch buckets and `SealedOperatorGroup`, with `ProducerId` in
  the claim store; moving it would turn `memory`'s one back-edge into a dozen. What is storage in
  it is the table shape, which `memory` already owns.
- *`model::types → core` — decided, partly cut here with the remainder recorded.* With `ScopeId` in `memory`,
  [`resolver.rs`](../../src/machine/model/types/resolver.rs) (`Scope`, `LexicalFrame`,
  `NameLookup`, `DeclarationSite`, `WriteOp`, `TypeWritePolicy`, `KError`) and
  [`typed_field_list.rs`](../../src/machine/model/types/typed_field_list.rs) (`KError`) are the
  whole surviving edge. Cutting it is a separate item once this one shows what the resolver
  actually needs.
- *Naming — open.* The family-generic frame door is named at implementation; the criterion is
  that no `core` type appears in `memory`'s signatures.

## Dependencies

**Requires:** none — a leaf refactor over the shipped frame door.

**Unblocks:**

- [Slot-shaped per-call scopes](../reduce_allocs/slot-shaped-per-call-scopes.md)
