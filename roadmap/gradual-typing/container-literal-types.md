# A container literal's element type

A field a program writes into a container literal stays reachable.

**Problem.** A list or dict carries the join of its elements' types
(`list_type` and `dict_type` in [`values`](../../src/values/admission.rs)), and
the load types a literal the same way. The join of two record types related by
width is the wider one (`join` in the
[type lattice](../../src/type_lattice/lattice.rs)), so
`[{x = 1, y = 2}, {x = 3}]` is a `LIST OF {x :Number}`: an overload over
`LIST OF :{x :Number}` is selected for it. Each element is read at that type,
since [a value's type is its surface](../../src/values/README.md#the-type-memo-and-satisfies),
so the literal prints `[{x = 1}, {x = 3}]`, equals
`[{x = 1, y = 9}, {x = 3}]`, and loses `y` when it is copied. Nothing ascribed
`y` away. A union keeps each member's own type — `[1, "a"]` is a
`LIST OF (Number | Str)` — but a record's width has no such counterpart.

**Acceptance criteria.**

- `[{x = 1, y = 2}, {x = 3}]` prints `[{x = 1, y = 2}, {x = 3}]`, and a copy of
  it keeps `y`.
- A container literal's static type is the type its value carries.
- A container ascribed `LIST OF {x :Number}` still reads each element at
  `{x :Number}`.

**Directions.**

- *What a literal's element type is — open.* A union of its elements' types
  that collapses no member into a wider one, or a join that keeps each field
  some member names. Either is a type-lattice design question that reaches the
  load's typing of every literal.
- *Whether it reaches every container the run builds — open.* A literal, a
  native's list and a joined dict key type may want one rule or two.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
