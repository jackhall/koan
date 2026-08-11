# Derived function reach

Derive a registered callable's — and operator group record's — reach from the
composition that placed it, retiring the empty-member mint asserted by fiat.

**Problem.** The registration doors `OverloadSeal::of_resident` and
`GroupSeal::of_resident`
([carrier_witness.rs](../../src/machine/core/carrier_witness.rs)) seal their
callable under a description hosted in its own scope's region with **no
members** — minted by `RegionBrand::seal_resident`
([arena.rs](../../src/machine/core/arena.rs)) — on the structural claim that a
function allocated into the very scope it captures borrows only its home. The
claim is never audited; a registration that ever embedded a foreign borrow
would under-pin. Three production `OverloadSeal` seals (keyworded `FN`, `OP`,
the builtin seeds via `register_function_direct`) and four `GroupSeal` seals
ride it. Deriving instead of asserting is blocked one step upstream:
`KFunction::alloc_captured`
([kfunction.rs](../../src/machine/core/kfunction.rs)) is a bare bump returning
an envelope-free reference, so at the seal site there is no composition to
derive from, and
[`Scope::store_function_cell`](../../src/machine/core/scope/reach.rs)'s own
composition takes that same empty mint as its left operand — it launders the
claim rather than superseding it. The type channel does not participate — a
`KType` is a `Copy` handle owning all its content, so a type binding carries
no reach to mint in the first place.

**Acceptance criteria.**

- A registered callable's reach description is composed from the operands
  that placed it — the captured scope's own coverage — not minted empty on an
  asserted claim; `RegionBrand::seal_resident`'s empty-member mint has no
  callable and no operator-group caller.
- `KFunction::alloc_captured` is a witnessed birth: its return carries the
  envelope the seal composes from, and no bare envelope-free `&KFunction`
  construction remains in production.
- `Scope::store_function_cell`'s wrapper composition takes the birth envelope
  as its function operand rather than re-minting an empty one.
- A `GroupSeal`'s carrier is rested from the group record's own birth
  envelope — a yoked construction whose `for<'b>` brand proves the record is
  region-pure — rather than sealed under a fresh empty mint.
- A test registering a callable through each production door (`FN`, `OP`, the
  builtin seeds) observes the derived description naming exactly the home
  region, and a test registering an operator group observes the yoke-derived
  description: hosted at home with no members — each structural claim held as
  a composed fact.

**Directions.**

- *Birth shape — decided.* `alloc_captured` is a fold/merge birth: the
  captured scope, the signature pre-minted at the same brand, and the body
  cross as one resident seed operand into a `merge_into` whose rank-2 brand
  proves residence and whose composition derives the description — hosted at
  home, home its one member. No new workgraph surface.
- *Group birth — decided.* The group record is born through
  `KoanRegionExt::yoke_branded` around `OperatorGroup::alloc` (which already
  re-homes every byte it stores), so its region-purity is the yoke brand's
  compile-time fact and the seal rests the birth envelope.

## Dependencies

**Requires:** none — the composition doors (`merge_into`, the fused
bind seals) already exist.

**Unblocks:** none tracked.
