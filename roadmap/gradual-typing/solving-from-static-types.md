# Static types of generic code

Generic code typed, solved and selected where the program loads, as far as the
static types reach, with the rest left to the call.

**Problem.** The load fixes four kinds of fact about generic code, and each
stops short in its own way.

*A name a run binds is an index.* The load pass reads it as a free
`quantified(index, bound)`, numbered per region
([`channel.rs`](../../src/elaborate/channel.rs)), so every binder above it
captures it:

- A run-bound name read while a `FOR ALL` group is open is left unknown.
- A `FOR ALL` callable whose type mentions an enclosing `FOR ALL`'s variables
  gets no parameter types and no return check (`seeded` and `returns` in
  [`statics.rs`](../../src/dispatch/statics.rs)). Inside
  `FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(…)`, a nested
  `FN FOR ALL #[Tee] :{t :Tee} -> Any` types `t` as `Any`, and under
  `FOR ALL #{Elt: Number}` a nested `FN FOR ALL #[Tee] :{t :Tee} -> Elt = #("no")`
  loads.
- A registration in a `FOR ALL` body whose type mentions the enclosing variables
  has a load-time registered shape that disagrees with the one elaborated where
  it is born. Running `EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Elt = #(…)` whose
  body registers `EXPR #(INNER y :Elt) -> Elt = #(y)`, as `OUTER 1`, fails the
  debug assertion in `staged` ([`knot/function.rs`](../../src/knot/function.rs)),
  whether or not the body uses `INNER`.

*A static type bounds from above only.* The load cannot tell that `1` carries
exactly `Number`, so no solve over static types pins a variable down: under
`EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str`, `PAIR 1 WITH 2` admits
at every call, and the load cannot see it. An `EVAL` is `Any`, whatever code it
runs, since `EVAL` is declared `-> Any`.

*Selection at load has three cases.* A use is `Full`, `Kept` or `Selected`
(`narrow` in [`statics.rs`](../../src/dispatch/statics.rs)):

- Only a lone closed candidate is selected. A lone rigid candidate admits at
  every call, and [`select::chosen`](../../src/dispatch/select.rs) assumes the
  candidate's registered type is closed.
- A use every candidate of which admits its arguments' static types is admitted
  and ranked at every call.
- An ambiguity the load can see is a fault at run time: two
  `EXPR #(PICK x :Number) -> Str` registrations load, and `PICK 1` faults.

*A generic call's group is not solved at load.* A call of a quantified callee is
typed by its declared return read through its bounds, so under
`EXPR FOR ALL #[Elt] #(ONLY x :Elt) -> Elt = #(x)`, `ONLY 1` is `Any`:
`(ONLY 1) + 1` admits at run time rather than selecting the builtin `+` where
the shape is built, and `(ONLY 1) + "a"` faults at run time rather than refusing
the load. The solution [`Collector::solve`](../../src/type_lattice/unify.rs)
gives over the arguments' static types is no bound on a call's:

- a variable at a contravariant position takes the least of its upper
  contributions, so an argument static type `FN :{x :Number} -> Null` solves
  `Elt` to `Number` in a slot `FN :{x :Elt} -> Null`, while the
  `FN :{x :Any} -> Null` a call carries solves it to `Any`;
- a variable no contribution reaches takes `Any`, not its declared bound, so a
  slot `(LIST OF Elt) | Null` solves `Elt` to `Number` over the static type
  `(LIST OF Number) | Null` and to `Any` over the `null` a call carries, even
  under `FOR ALL #{Elt: Number}`;
- an upper bound on a variable, substituted into a return that names it at a
  contravariant position, is no bound on the return: `Elt` bounded by
  `Number | Str` at load and solved to `Number` at the call gives
  `FN :{y :(Number | Str)} -> Null` for a return `FN :{y :Elt} -> Null`, which
  the call's `FN :{y :Number} -> Null` does not lie under.

**Acceptance criteria.**

Lexical variables:

- A name only a run binds is read at load as a lexical variable, numbered where
  it is declared, the names of every enclosing body first, and no load-time
  type holds a free `Quantified`.
- A run-bound name read under an open `FOR ALL` group is rigid: inside
  `FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(…)`, the nested
  `FN FOR ALL #[Tee] :{t :Tee} -> Elt` has a rigid load-time type naming `Elt`.
- A rigid load-time type is read where it runs by one substitution by level.
- Typing a quote's code a second time interns no node.
- A candidate whose slot names an enclosing body's variable is admitted with no
  solve.
- In a `FOR ALL` callable nested in another `FOR ALL` body, each parameter has
  its declared type as its static type, and the return check applies: the nested
  `FN FOR ALL #[Tee] :{t :Tee} -> Elt = #("no")` under `FOR ALL #{Elt: Number}`
  refuses the load.
- A registration in a `FOR ALL` body whose type mentions the enclosing variables
  has the registered shape at load that it has where it is born, its variables
  substituted: `OUTER 1` above runs in a debug build.

Static types:

- A static type is an interval. A literal, a quote, a `FN` and a type
  expression the load knows are exact; a list, dict or record literal has the
  ends its parts' ends build; and a static type whose upper end is `Number`,
  `Str`, `Bool` or `Null` is exact.
- A static type leaving a quote's code, an `EVAL`'s, or crossing into it, a `$`
  name's, is read through `bound_above`.
- An `EVAL` of code traced to a written quote — its operand, or a name `LET`
  binds to one — is at most the code's last statement's upper end: after
  `LET q = #(1 + 2)`, `(EVAL q) + 1` selects the builtin `+` where the shape is
  built.
- In a debug build, every finished value's carried type lies within its node's
  static type.

Verdicts:

- Each candidate of a keyworded use has one verdict at load. It is *never* when
  some slot meets its argument's upper end at `Never`; *always* when every class
  admits whatever a call carries within its arguments' static types — a slot
  that is a variable of its own class alone, named by no other slot of the
  class, admitting under the variable's bound, and a class whose arguments
  naming its own variables are exact admitting when its static solve does; and
  *maybe* otherwise.
- Under the `PAIR` above, `PAIR 1 WITH 2` is selected at load, and `PAIR a WITH b`
  over parameters `a :(Number | Str)` and `b :(Number | Str)` is not.
- A *maybe* candidate that an *always* one strictly outranks at the first class,
  both closed, is dropped at load: beside `EXPR #(SHOW x :(LIST OF Number)) -> Str`
  and `EXPR FOR ALL #[Elt] #(SHOW x :(LIST OF Elt)) -> Str`, a use `SHOW xs` of a
  parameter `xs :(LIST OF Number)` is selected at load.
- A use with no *maybe* candidate, holding one candidate or only closed ones, is
  ranked at load, and its call runs the winner without admitting or ranking.
- Such a use none of whose candidates ranks first refuses the load at
  `path:line:col`: two `EXPR #(PICK x :Number) -> Str` registrations and
  `PICK 1` refuse it.
- A call admits only its *maybe* candidates, and runs what selection over the
  full list would.
- In the body of `EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Elt = #(…)`, a use
  `INNER x` of a lone `EXPR #(INNER y :Elt) -> Elt = #(y)` declared there is
  selected at load and runs what full selection would, while under
  `FOR ALL #{Elt: Number}` the use `INNER 1` is not selected.

Intervals:

- The unifier reports, per variable, an interval of a lower and an upper end,
  and a variable no contribution reaches solves to its declared bound.
- A lattice law holds over the generators: for carried types each within its
  static interval, every solution over the carried types lies in the interval
  reported over the static ones.
- A keyworded use of a quantified candidate, and a call by name of a quantified
  function, types the declared return through the intervals: a variable's upper
  end at a covariant position, its lower end at a contravariant one.
- Under the `ONLY` above, `ONLY 1` has the static type `Number`,
  `(ONLY 1) + 1` selects the builtin `+` where the shape is built, and
  `(ONLY 1) + "a"` refuses the load.
- Under
  `EXPR FOR ALL #[Elt] #(HANDLER f :(FN :{x :Elt} -> Null)) -> :(FN :{x :Elt} -> Null) = #(f)`,
  a use whose argument has the static type `FN :{x :Number} -> Null` has the
  static type `FN :{x :Number} -> Null`.

**Directions.**

- *How a name a run binds is represented — decided.* A lexical variable: a
  lattice node of its own, positional by its level along the lexical chain and
  carrying its name, that no binder captures and no solve binds. `Quantified`
  stays bound by its binder, and `AbstractType` is not used, since
  [modules](../rewrite/modules.md) removes it. It is a type-lattice change, approved as
  one.
- *How far an interval goes — decided.* Both ends. The collector reports the
  interval, and the lattice reads a type through intervals by variance, of which
  `bound_above` is the read at `[Never, bound]`. No variable carries a lower
  bound of its own. A collector holds its group's bounds from the start. It is a
  type-lattice change, approved as one, and so is judging a candidate by class
  in `ranking.rs`.
- *Static types with a lower end — decided.* Every static type is an interval,
  and an exact one is a point. Where the arguments naming a variable are exact,
  the static solve is the call's own, so the variable is solved to a point.
- *Ranking at load — decided.* A use with no *maybe* candidate is ranked where
  the shape is built when it holds one candidate or only closed ones, and a
  certain ambiguity refuses the load, as a use no candidate admits does. Several
  rigid candidates are ranked at the call: a relation over a lexical variable
  holds at every run, but its absence need not, so `INNER y :Elt` over a rigid
  `Elt` bounded by `Number` strictly outranks `INNER y :Number` at load and ties
  with it where `Elt` is `Number`.
- *A maybe an always outranks — decided.* Dropped where an *always* candidate
  strictly outranks it at the first class and both are closed: whenever it
  admits, the *always* one admits too, and elimination drops it at that class.
- *Load-refusal columns — decided.* A load refusal is located at its node's
  group, so a single-statement body `#(x)` reports the column of its `(`, as
  every node-level error does.
- *What an `EVAL` is typed by — decided.* The code it runs, where the load
  traces that code to a written quote. Code names extend the tracing
  ([code names](../metaprogramming/code-names.md)), and code composed at run time is typed where it
  is built ([code splicing](../metaprogramming/code-splicing.md)), so neither is traced here.
- *A call solved from its static types — deferred* to
  [calls solved from their static types](static-solutions.md).

## Dependencies

**Requires:** none.

**Unblocks:**

- [Calls solved from their static types](static-solutions.md) — the static
  solve and its verdicts.
- [Solving to the least instance](least-instance-solving.md) — the interval law
  and its property tests.
- [Value ascription](value-ascription.md) — exact static types.
