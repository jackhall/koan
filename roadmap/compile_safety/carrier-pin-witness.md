# Carrier pin witness

Tie a carrier's erased reach to the pin a reader presents, so co-location is a
compile obligation rather than a discipline.

**Problem.** [`Carrier`](../../workgraph/src/witnessed/carrier.rs) stores its
reach as an erased reference and re-anchors it only under an externally
supplied pin. Nothing in the type ties the reach's backing arena to that pin:
the open doors — `Sealed::open_at`
([witnessed.rs](../../workgraph/src/witnessed.rs)) and
`Carrier::upgrade_bundle` — bound the pin as `Pin: Witness`, a bare marker
trait with no region projection, and their bodies discard the argument
(`let _ = pin;`). `NoPins` satisfies the bound while pinning nothing. A read
re-anchoring under a pin narrower than the reach's backing arena would dangle.
Every `Sealed::open_at` / lift reader rides this; the exposure is bounded
because a resting entry is one `Sealed` fusing value and reach and the open
doors take coverage by signature, but the residual bound is discipline.

**Acceptance criteria.**

- The open doors (`Sealed::open_at`, `Carrier::upgrade_bundle`) state a
  compile-checked relation between the presented pin and the carried reach's
  backing region; a `compile_fail` fixture opening a `Sealed` at a pin over an
  unrelated region fails to compile.
- No open door's body discards its pin argument — the pin participates in the
  signature relation.
- Every reader that opens with no pin of its own routes through a separately
  named residence-only door carrying its outside-the-type justification (the
  `read_resting` family), not through the general open.

**Directions.**

- *Bound shape — open.* (a) Rebound the open doors by `WitnessRegion`
  ([witnessed.rs](../../workgraph/src/witnessed.rs)), which already carries the
  region projection, with an equality constraint to the carrier's profile;
  (b) parameterize `Sealed` over its backing region so the relation is carried
  by the type itself. Recommended: (a) — the projection exists and the change
  stays on the door signatures.
- *`NoPins` readers — open.* (a) Keep `NoPins` but only on the named
  residence-only doors; (b) delete `NoPins` and make each residence-only site
  present its step's real coverage. Recommended: (a) — the `read_resting`
  family's justification is genuinely outside the type.

## Dependencies

**Requires:** none — the `WitnessRegion` projection already exists.

**Unblocks:** none tracked.
