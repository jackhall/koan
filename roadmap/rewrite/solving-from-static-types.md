# Solving a group from static types

A generic call typed by its callee's return at the solution its arguments'
static types give.

**Problem.** The load's [static types](../../src/dispatch/README.md#static-types)
type a call of a quantified callee by its declared return read through its
bounds, so under `EXPR FOR ALL #[Elt] #(ONLY x :Elt) -> Elt = #(x)`, `ONLY 1` is
`Any`: `(ONLY 1) + 1` admits at run rather than selecting the builtin `+` where
the shape is built, and `(ONLY 1) + "a"` faults at run rather than refusing the
load. Solving the
group from the arguments' static types instead is unsound under the
[unifier](../../src/type_lattice/README.md#the-unifier-collects-it-does-not-bind)
as it stands, since its solution does not grow with its arguments:

- a variable at a contravariant position takes the least of its upper
  contributions, so an argument static type `FN :{x :Number} -> Null` solves
  `Elt` to `Number` in a slot `FN :{x :Elt} -> Null`, while the
  `FN :{x :Any} -> Null` the run carries solves it to `Any`;
- a variable no contribution reaches takes its bound, so a slot
  `(LIST OF Elt) | Null` solves `Elt` to `Number` over the static type
  `(LIST OF Number) | Null` and to `Any` over the `null` the run carries.

**Acceptance criteria.**

- The unifier reports, per variable, whether its solution grows with its
  arguments: solved from covariant contributions alone, at positions every
  argument lying under the slot reaches.
- A lattice law holds over the generators: for argument types each under
  another's, a solution the report marks growing solves the first no higher than
  the second.
- A keyworded use statically selecting a quantified candidate, and a call by
  name of a quantified function, has as its static type the declared return
  substituted by the static solution for every growing variable and read through
  `bound_above` for the rest.
- Under the `ONLY` above, `ONLY 1` has the static type `Number`,
  `(ONLY 1) + 1` selects the builtin `+` where the shape is built, and
  `(ONLY 1) + "a"` refuses the load.

**Directions.**

- *Report or restrict — open.* The report can ride the collector's `solve`, or a
  separate solve can refuse outright where a variable would not grow.
  Recommended: the report, so one solve serves the run and the load.

## Dependencies

**Requires:** none.
