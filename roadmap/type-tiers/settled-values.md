# Settled values

Two bits on a value saying where its memos already resolve, so an adoption
walk stops there.

**Problem.** An adoption — `adopt_value` in `src/values/crossing.rs` — makes
every memo a value reaches resolve in its destination by walking the value:
each part, hidden parts included, and every value and handle its knots hold,
since a part keeps its own memo under `Any`, a union or a carrier. Its one
cutoff is `received`: the values the load lists per site as a parameter, a
capture or a builtin `ATTR` member read of one, which the walk does not enter
(`Caching.received`, computed in `src/dispatch/statics/caching.rs`). A
received part reached by any other route — a `MATCH` arm, a call's result —
is walked whole, so a body handing out a wrapper over such a part walks
O(depth) per call. At an invariant site under `routes` (`ascribe` in
`src/dispatch/evaluate.rs`) the same door adopts the operand and the view into
the lent branch with no `received` at all, on every call: a module built over
a received chain is walked to the chain's end each hop, and `wrapped_peeks` in
`src/dispatch/tests/caching.rs` measures 448 handles adopted at eight hops and
3328 at thirty-two, on either placement.

The value layout leaves no byte for a flag: the `Value` word is 24 bytes with
no addressable bits, `Tagged` and the knot `Node` have no padding, and `Node`
is asserted at 64 bytes. Every composite header and every node arm carries a
`Weight` (`src/values/weight.rs`), a saturating byte count whose top bits no
value reaches, built from its parts at construction and read only through
`bytes()`.

**Acceptance criteria.**

- `Weight` carries two bits in its top, masked out of `bytes()` and of
  equality: **root-closed**, every memo the value reaches is the root's, set
  at construction from the parts; **settled**, every memo it reaches resolves
  in the root or its home region's branch, set at construction by a porous
  writer over settled parts and on every value an adoption into a region
  finishes.
- Every composite header and every knot node holds `Cell<Weight>`; `resident`
  in `src/memory/substrate.rs` bounds by `!needs_drop` as `Writer::fill`
  does; `Node` stays 64 bytes and `Knotted` stays `Copy`.
- Exits and invariant sites adopt through one door, which does not enter a
  part with either bit set, and whose debug check walks with the same cutoff.
- A clear bit is always safe and a set bit is true: a restamp (`with_type`,
  `peel`, `restamped`) recomputes both from the new type; a copy
  (`Circular::copied`, each node's `rebuilt`) recomputes them at its
  destination; a knot's bits sit on its root node.
- `Caching.received`, the statics computing it and the walk's `same_word`
  scan are deleted, and the tests over received values pass on the bits.
- A `Fresh` loop handing a module built over its link each hop to a `Fresh`
  caching helper holds adoptions flat per hop; so does a loop handing out a
  wrapper over a received part reached through a `MATCH` arm.
- The `Cell` write through a laid header is on the Miri slate.

**Directions.**

- *One door for exits and invariant sites* — decided. The door's contract is
  that every memo a value reaches resolves where the value's readers read: a
  root type everywhere, a settled type in its home region's branch, which
  every chain over a value in that region links. No site adopts a prefix of
  the value on its own account.
- *Where the bits live* — decided. `Weight`'s top two bits, saturating below
  them: free on every header and node, one masking site, and the same kind of
  memo the weight is. Rejected: a byte in the headers, free in `Record`,
  `List` and `Dict` but 16 bytes on `Tagged` and on every node; bits in the
  type's digest, which every lookup and comparison would have to mask.
- *The settled write* — decided. `Cell<Weight>`: the bump already allows
  interior mutability (`Writer::fill` asserts `!needs_drop`, not `Copy`); the
  headers and nodes drop `Copy`, which nothing copies by value after the lay
  but `Tagged::with_type`, rewritten as the other restamps are.
- *Leaves and retypes* — decided. A scalar, a `Str`, a `TypeValue` and a
  `Retyped` carry no bits: none has parts of its own, and a `Retyped` is
  adopted at its memo and then read through to its member's bits.

## Dependencies

**Requires:** none — the walk, the headers and the bump stand.

**Unblocks:**

- [A tenant's own lend](tenant-lend.md) — a keyed `Shares` body's invariant
  sites walk their operands into the lent branch on every call, affordable
  only past a settled part.
