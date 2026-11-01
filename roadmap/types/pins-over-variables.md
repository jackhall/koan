# A pin over a variable

A static type holding a signature application that pins a head parameter to a
path or a variable.

**Problem.** The type lattice reads a variable a `SignatureApply` pin names
through its bounds at the enclosing variance — `read_through` and
`bound_above` in [substitute.rs](../../lattice/src/types/substitute.rs) visit a
pin at the variance of the application holding it — while the order compares
two pins of one parameter invariantly (`application_under` in
[signatures.rs](../../lattice/src/types/signatures.rs)). So
`S WITH {Elt = Elt}` read above is `S WITH {Elt = Any}`, which lies above no
instance but the one at `Any`.
[`signature_member`](../../src/elaborate/members.rs) reads pins through
`bound_above` too. Dispatch therefore keeps no such pin in a static type, and
pins a parameter to its path only while it reads a member
([pins are read, not stored](../../src/dispatch/README.md#paths)). Two
programs pay for it:

- A module retyped to `Counter` is re-stamped at `Counter` pinned at what
  *fits* solves each parameter to, which each run computes afresh, so over
  `h :Counter` the load reads `h` at most `Counter` and `h.zero` at most
  `Carrier` ([static types](../../src/dispatch/README.md#static-types)), exact
  only where it knows the module's signature and pins it with the run's own
  function.
- Under `EXPR FOR ALL #[Elt] #(USE x :Elt p :(Mapper WITH {Elt = Elt})) -> Any`,
  over the `Mapper` and `two` of the dispatch test
  `a_path_reads_through_every_pin_of_its_declared_application`, `USE 1 two` is
  refused with no overload admitting it, though `two` fits
  `Mapper WITH {Elt = Number}`. Why the solve fails is not yet diagnosed; its
  failing slot is a pin over the callable's own variable.

**Acceptance criteria.**

- A type holding a pin over a variable, read through its bounds from above,
  lies above every instance of it under the order, and read from below lies
  under every instance; a law in the
  [property suite](../../lattice/src/types/tests/properties.rs) says so.
- A static type may hold an application pinning a head parameter to a path:
  over `h :Counter` the load types `h` exactly at `Counter` pinned at
  `h.Carrier`, and `h.zero` exactly at `h.Carrier`.
- `USE 1 two` is admitted, solving `Elt` to `Number`.

**Directions.**

- *How a pin's variable reads through its bounds — open.* A third variance,
  invariant, at which a variable reads as its point where its interval is one,
  and otherwise a read from above drops the pin and a read from below is
  `Never`; or each head parameter ordered at the variance its signature's
  members read it at, so a parameter read only covariantly orders its pins
  covariantly, which changes the order and its laws. Recommended: the first.
  Either is a change to the type lattice, a flagged exception needing an OK.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
