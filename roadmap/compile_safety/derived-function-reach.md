# Derived function reach

Derive a registered callable's reach from the composition that placed it,
retiring the empty-member mint asserted by fiat.

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
  callable caller.
- `KFunction::alloc_captured` is a witnessed birth: its return carries the
  envelope the seal composes from, and no bare envelope-free `&KFunction`
  construction remains in production.
- `Scope::store_function_cell`'s wrapper composition takes the birth envelope
  as its function operand rather than re-minting an empty one.
- A test registering a callable through each production door (`FN`, `OP`, the
  builtin seeds) observes the derived description naming exactly the home
  region — the structural claim held as a composed fact.

**Directions.**

- *Birth shape — open.* (a) Make `alloc_captured` a fold/merge birth whose
  operands are the captured scope's coverage, so "borrows only its home" is a
  composed member set; (b) keep the bare bump and audit the claim at seal time
  with a runtime walk. (b) is rejected as an end state — a runtime residence
  walk is what this project retires; at most a stopgap with this item left
  open. Recommended: (a).
- *Seal fusion — open.* Both non-seed production sites mint the overload seal
  and immediately call `store_function_cell` (`fn_def/finalize.rs`,
  `op_def.rs`); a fused door returning both products off one composition would
  collapse the two mints into one derived reach. Worth taking only after the
  birth ships — decide when the birth's envelope shape is settled.

## Dependencies

**Requires:** none — the composition doors (`merge_into_placing`, the fused
bind seals) already exist.

**Unblocks:** none tracked.
