# Home lives in the reach description

A value's home region is an ordinary member of its minted reach description and
is recorded nowhere else — no borrows-home bit, no envelope-side residence probe.

**Problem.** Home is recorded three ways.

*In the description*, as an ordinary `Weak` member — the shipped rule, and the one
the composition verbs apply.

*In a bit.* Every fold engine that builds a witnessed product
(`map_pinned_placing`, `merge_pinned_placing`, `transfer_into_placing`) composes
the product's witness from the fold's *other* operands, so it is structurally
blind to the value the closure just built. A substrate carrier (`Record` / `List`
/ `Dict` / `Tagged` / `Wrapped`) born that way under-reports its own self-borrow
into its birth region, and
[`force_substrate_borrows_host`](../../src/machine/core/carrier_witness.rs)
patches it after the fact by rebuilding the witness as
`CarrierWitness::new(true, None)` — the borrows-home bit set, the reach reference
empty. That state is unrepresentable as membership: with no description there is
no member set to hold home. So
[`Carrier::borrows_host`](../../workgraph/src/witnessed/carrier.rs) is not the
redundant mirror of an ordinary member it looks like — it is the sole record of
the home borrow for every value born this way, and `Carrier::is_empty` reads it
to decide whether a top-level root gets relocated into the run region before its
per-call frame releases
([interpret.rs](../../src/machine/execute/runtime/interpret.rs)). The force site
cannot mint instead of setting a bit, because it holds only the producer's frame
owner (`&Rc<FrameStorage>`), not a `RegionHandle` to allocate a description
through.

*In a side channel on the envelope.* `Delivered::with_home_region`
([delivered.rs](../../workgraph/src/witnessed/delivered.rs)) hands back the
residence identity the envelope's container supplied, and
`PinBundle::any_member_region` ([reach.rs](../../workgraph/src/witnessed/reach.rs))
scans a bundle for a region satisfying a predicate. Koan reads both:
`product_still_borrows` and `clone_still_borrows`
([carrier_witness.rs](../../src/machine/core/carrier_witness.rs)) decide which
region a copying relocation releases by asking `with_home_region` whether the
region handed over *is* home; [`lift.rs`](../../src/machine/execute/lift.rs) and
[`scope/reach.rs`](../../src/machine/core/scope/reach.rs) gate their adopt seams
the same way; [`arena/residence.rs`](../../src/machine/core/arena/residence.rs)
evidences a foreign borrow through `any_member_region`. Three records of one
fact, each able to disagree with the other two — and while the embedder boundary
holds everywhere else (`PinBundle` is crate-private, owned pins cross only as the
opaque `StepCoverage`), these two probes are the hole in it: Koan still recovers
a producer region from the library rather than being handed it.

**Acceptance criteria.**

- A substrate carrier born at a fold door references a reach description that
  names its birth region as an ordinary member, minted at the birth site.
- `Carrier::borrows_host`, its accessor, and every `borrows_host` argument
  threaded through `Carrier::new` are deleted; `Carrier` is the reach reference
  alone.
- `Carrier::is_empty` and `Carrier::has_reach_members` answer the same question,
  and one of them is deleted.
- `force_substrate_borrows_host` is deleted; no site rebuilds a witness after a
  fold to correct its reach.
- `Delivered::with_home_region` and `PinBundle::any_member_region` are deleted,
  and no koan source file names either.
- A copying relocation decides what it releases from the built product and the
  minted description alone: `product_still_borrows` and `clone_still_borrows`
  take the home identity from the container verb that already supplies it, or
  cease to exist, rather than probing the envelope for it.
- A test builds a substrate value at a fold door, asks its opened carrier whether
  it reaches the birth region, and gets true without consulting any bit.
- The Miri audit slate is leak-free and UB-free with the bit and both probes
  gone.

**Directions.**

- How the birth site gets its mint target — *open.* The force site holds the
  producer's `Rc<FrameStorage>`, not a `RegionHandle`. Options: (a) thread the
  destination handle into the fold verbs that can produce a substrate, so the
  engine mints the self-member itself and no correction pass exists; (b) give the
  koan-side force a handle argument and keep it as an explicit post-fold mint;
  (c) have `ReachDescription::mint` accept the destination as a self-member so
  the engine's existing compose call covers it. Recommended: (a) — it removes the
  correction pass rather than re-plumbing it, and matches the composition rule
  that a fold's product reach is derived, never patched.
- What replaces the `with_home_region` probe — *open.* The predicate needs to
  know, per region the library hands over, whether that region is the value's
  home. Options: (a) the container verb passes the home owner alongside each
  region, so the predicate compares two values it was given and probes nothing;
  (b) the predicate takes only the product and the region, and home membership is
  read off the minted description — viable only once the first criterion holds,
  since a fold-born carrier has no description today. Recommended: (a) — it is
  the rule the rest of the embedder boundary already follows (the container
  supplies the home owner each verb needs), and it does not depend on the
  birth-site mint landing first.
- Whether `any_member_region` survives its koan caller — *open.*
  `arena/residence.rs` uses it to evidence a foreign borrow during a move-in
  audit, which is not a home question at all. Either that call site moves to a
  membership query on the description, or the probe is renamed and kept for the
  audit path with only the home-recovery use deleted. Settle which before
  writing the deletion.
- Interaction with the self rule — *decided.* A region never pins itself, and
  that rule applies to the owned `PinBundle` alone; the *description* records
  home as an ordinary member. A birth-site mint follows the same split: home
  enters the description, not the region's own bundle.
- Whether `is_empty` or `has_reach_members` survives — *open.* They differ only
  in the bit. `has_reach_members` names what remains; `is_empty` is what the
  current call sites read.

## Dependencies

**Requires:** none — the carrier substrate it builds on has shipped.

**Unblocks:** none — leaf.
