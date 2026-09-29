# Static types of generic code

A generic call typed by its callee's return at the solution its arguments'
static types give, and generic code nested in a `FOR ALL` body typed and
selected at load.

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

Generic code nested in a `FOR ALL` body loses two more things at load:

- A `FOR ALL` callable whose type mentions an enclosing `FOR ALL`'s variables
  gets no parameter types and no return check (`seeded` and `returns` in
  [`statics.rs`](../../src/dispatch/statics.rs)). Its load-time type numbers the
  enclosing region's variables with the same `Quantified` indices as its own, so
  inside `FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(…)`, a nested
  `FN FOR ALL #[Tee] :{t :Tee} -> Any` types `t` as `Any`. Under
  `FOR ALL #{Elt: Number}`, a nested `FN FOR ALL #[Tee] :{t :Tee} -> Elt = #("no")`
  loads.
- Static selection only selects a closed candidate (`narrow` in
  [`statics.rs`](../../src/dispatch/statics.rs)). A registration in a `FOR ALL`
  body whose type mentions the enclosing variables is rigid, and a lone use of it
  still admits at run. Admission read through the variables' bounds cannot select
  it soundly: a `Number` argument meets `Elt` bounded by `Number`, yet the run may
  bind `Elt` narrower. [`select::chosen`](../../src/dispatch/select.rs) also
  assumes the candidate's registered type is closed.
- Such a registration's load-time registered shape disagrees with the one
  elaborated where it is born. Running
  `EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Elt = #(…)` whose body registers
  `EXPR #(INNER y :Elt) -> Elt = #(y)`, as `OUTER 1`, fails the debug assertion
  in `staged` ([`knot/function.rs`](../../src/knot/function.rs)), whether or not
  the body uses `INNER`.

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
- In a `FOR ALL` callable nested in another `FOR ALL` body, each parameter has
  its declared type as its static type, with the enclosing variables read through
  their bounds, and the return check applies: the nested
  `FN FOR ALL #[Tee] :{t :Tee} -> Elt = #("no")` under `FOR ALL #{Elt: Number}`
  refuses the load.
- A registration in a `FOR ALL` body whose type mentions the enclosing variables
  has the same registered shape at load as where it is born: `OUTER 1` above runs
  in a debug build.
- In the body of `EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Elt = #(…)`, a use
  `INNER x` of a lone `EXPR #(INNER y :Elt) -> Elt = #(y)` declared there is
  selected at load and runs what full selection would, while under
  `FOR ALL #{Elt: Number}` the use `INNER 1` is not selected.

**Directions.**

- *Report or restrict — open.* The report can ride the collector's `solve`, or a
  separate solve can refuse outright where a variable would not grow.
  Recommended: the report, so one solve serves the run and the load.
- *Separating nested variables — open.* The nested callable's type can be read
  with the enclosing variables substituted by their bounds, which needs a
  substitution that tells them apart from its own; or the load can number a
  nested region's variables after its enclosing region's.
- *Rigid admission — open.* A rigid candidate is selected when its slots admit
  the arguments' static types with each rigid variable held opaque, so only
  `Elt` meets `Elt`. `chosen` then reads the candidate's registered type as the
  run instantiates it.
- *Load-refusal columns — open.* A load refusal is located at its node's group,
  so a single-statement body `#(x)` reports the column of its `(`, as every
  node-level error does. It can stay there, or every node-level error can point
  at its node's first token.

## Dependencies

**Requires:** none.
