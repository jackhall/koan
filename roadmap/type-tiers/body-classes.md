# Body classes

A porosity flag on every root type, and every body classified where the
program loads as inert, sealed or porous by the sites it holds.

**Problem.** The static pass ([statics.rs](../../src/dispatch/statics.rs))
fixes an interval per value expression and binder and says nothing about
whether a site can hold a run-built type, and nothing classifies a body. The
lattice's `Flags` byte ([stored.rs](../../lattice/src/types/stored.rs)) folds
four probe answers from a node's children at intern, none of which says
whether a slot admits a carrier: `CARRIER` marks a type that holds one, the
value side. So [type branches](type-branches.md) has no load-time fact by which
to skip the branch probe at a site that can only hold a root handle, and
[shape branches](shape-branches.md) has no class by which to choose the bodies
that lay a branch.

**Acceptance criteria.**

- The lattice folds porosity at intern as two bits per node, swapped under a
  function type's parameter, and a root handle answers whether it is porous in
  one probe. A law in
  [tests/properties.rs](../../lattice/src/types/tests/properties.rs) holds: a
  type holding a carrier lies under, or fits, a type only where that type is
  porous.
- Every body is classified at the load, in a cell on its shape: inert where no
  site's static type is porous and the body holds no minting site, porous
  where its declared return or an argument slot of a call it makes is porous,
  sealed otherwise. A test program per class pins the class the pass gives it,
  among them a body whose only porous site is the result of a call.
- A sealed body's lays-a-branch bit is fixed beside its class: sealed, its
  return placed `Shares` by `placement_of`, and at least one minting site.
- A measurement over the example programs counts the bodies per class and the
  minting sites each sealed `Shares` body holds.

**Directions.**

- *The fold's variance — decided.* Two bits, covariant and contravariant, each
  node's the union of its children's with the pair swapped under a function
  type's parameter, the root read at its covariant bit. A variable and an
  unpinned head parameter set both; `Any` sets the covariant bit and `Never`
  the contravariant one ([type tiers](../../design/type-tiers.md#porosity)).
- *The class as a fold — decided.* Accumulated in the static pass's per-shape
  record as it walks the parts, candidate slot types, view sites and type
  binders it already visits, and written with the shape's statics; no second
  pass.
- *Inert by sites — decided.* No site whose static type is porous, rather than
  an enumeration of entries, so a call's porous result counts without being
  listed.
- *The minting sites — open.* A view ascription and a `NEWTYPE` in a callable
  body over a run-bound name are the two doors the design names; whether a
  run-built code shape mints is unsettled. Recommended: those two, and the
  measurement says whether a third is missed.

## Dependencies

**Requires:** none — load-time, and tested standalone.

**Unblocks:**

- [Shape branches](shape-branches.md) — the class decides which bodies lay a
  shape branch.
