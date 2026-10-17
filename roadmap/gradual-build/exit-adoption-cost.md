# The cost of an exit's adoption

**Problem.** A caching body adopts at every exit
([type tiers](../../design/type-tiers.md#the-scratch-branch-as-a-cache)):
a keyworded call walks its callee and its argument record and adopts its
contract's three handles, a call by name adds a contributed type per
parameter, a frame's end walks its result. The walk, `carried` in
`src/values/crossing.rs`, hands over each part's own memo *before* reading
the part's bits, so a part the settled cutoff skips still costs one chain
lookup; and it lays a bumpalo `Bump`, a stack and a knot set per walk, with a
second `Bump` for the parts an exit marks settled, all before looking at the
top value, so an exit of a value whose bits already meet the contract pays
two chunk allocations to learn it had nothing to do. Measured over the
caching suite's programs (`scratch/eviction-and-handle-bits.md`): 4–9 exit
walks per loop hop, each handing over 2–5 handles that every time resolve at
once, two thirds of the parts reached skipped on their bits — and no node
laid.

**Acceptance criteria.**

- At a `Region` exit a part whose bits meet the contract hands over nothing,
  its own memo included: both bits promise that memo resolves in the root or
  the home region's branch, which the exit's chain reaches. At a `Warm`
  destination the part's own memo is still handed over, since the lent branch
  is not where the value lies. A `Retyped` header, which carries no bits of
  its own, still hands over its own memo and stops at its member's bits.
  `an_exit_settles_what_it_walked` in `src/values/tests/settled.rs` asserts
  that a settled value's second exit adopts nothing.
- The walk reads the top value's bits before it lays anything: an exit of a
  settled or root-closed composite allocates nothing.
- The walk's stack, knot set and settled list live in the step's scratch
  allocator, not in a `Bump` of their own.
- The adoption counters of the measurement — probes, hits and nodes laid in
  `adopt_digest`; walks per destination; parts entered and skipped — ship
  under the `counters` feature, and a test reproduces the per-hop table over
  the caching suite's programs.

**Directions.**

- *One walk per call* — open. A keyworded call's exit walks the callee, then
  the argument record, then adopts the contract; one walk over all three
  shares a stack and a knot set. Whether the saving is worth a door that takes
  several values is settled by the counters above.

## Dependencies

**Requires:** none — the adoption cutoff stands.

**Unblocks:** none.
