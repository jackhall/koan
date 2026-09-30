# Value ascription

`:!` over any value, and a declared parameter as an ascription.

**Problem.** `:!` and `:|` ascribe modules, which [modules](../rewrite/modules.md)
evaluates, and nothing states a value's type where the value is written. An
`EVAL`'s value stays at most `Any` at load, and widening `[["a"], [1]]` to
`LIST OF (LIST OF (Number | Str))` takes a function with that declared return.

A frame's value is retyped to its declared return
([frames](../../src/program/README.md#frames-contracts-and-tails)), but an
argument is not retyped to its parameter's declared type, so a body dispatches
on its arguments' contents rather than on its declarations. Beside
`EXPR #(WHICH x :(LIST OF Number)) -> Str = #("numbers")` and
`EXPR #(WHICH x :(LIST OF Any)) -> Str = #("any")`, a function
`EXPR #(SHOW x :(LIST OF Any)) -> Str = #(WHICH x)` answers `SHOW [1]` with
`"numbers"`. At load a parameter is at most its declared type, never exactly it.
A frame one tail hop reaches ends under its own callee's return, so after
`EXPR #(INNER) -> :(LIST OF Number) = #([1])` and
`EXPR #(OUTER) -> :(LIST OF Any) = #(INNER)`, `OUTER` carries `LIST OF Number`.

[`Value::retyped`](../../src/values.rs) stamps a container only against a node
of its own kind, so a union never retypes one: a declared
`(LIST OF Any) | Null` leaves `[1]` at `LIST OF Number`. A tagged value against
a union takes the first member naming its constructor, whether or not the value
lies under that member. A knot's data node is never retyped.

**Acceptance criteria.**

- `e :! T` evaluates over any value but a module: a value satisfying `T` is the
  result, retyped to `T`, and any other is a fault naming `T`.
- Where `e`'s static upper end lies under `T`, the run checks nothing; where the
  two meet at `Never`, the use refuses the load.
- A retype reads `T` member by member: the value takes the meet of `T`'s members
  of its own kind — a list, dict or record node for a container, a node naming
  its constructor for a tagged value — that it lies under, and keeps its own
  type where `T` has none. After `LET Loose = :((LIST OF Any) | Null)` and
  `LET Wide = :((LIST OF (Number | Str | Bool)) | (LIST OF (Number | Str | Null)))`,
  `[1] :! Loose` carries `LIST OF Any`, `[1] :! Wide` carries
  `LIST OF (Number | Str)`, and `[1] :! Any` carries `LIST OF Number`.
- A tagged value ascribed to a union of two applications of its family carries
  the application it lies under.
- A knot's data node is retyped as a plain value of its kind over its cells, so
  a cyclic list passed to `SHOW` answers `"any"`.
- A declared parameter is an ascription: a keyworded call and a call by name
  retype each argument to its parameter's declared type, with the call's
  type-parameter solution substituted, so `SHOW [1]` above answers `"any"`.
- A frame's value is checked against its own callee's return and retyped to the
  outermost return of the tail hops that reached it, so `OUTER` above carries
  `LIST OF Any`, and a miss still names the innermost callee.
- At load, `e :! T` and a parameter declared `T` are exactly `T` where `T` is a
  list, dict or record type, and at most `T` otherwise. Under
  `EXPR FOR ALL #[Elt] #(FLAT rows :(LIST OF (LIST OF Elt))) -> :(LIST OF Elt) = #(…)`,
  a use `FLAT rows` of a parameter `rows :(LIST OF (LIST OF (Number | Str)))` is
  selected at load.

**Directions.**

- *Spelling — decided.* `:!` extends to every value and keeps one meaning: check
  a value against a type and view it at that type, a module's type being its
  signature. `:|` stays a module's, since sealing a value is what a `NEWTYPE`
  does. Over a module, `:!` runs the view door, which [modules](../rewrite/modules.md)
  evaluates.
- *A declared parameter is an ascription — decided.* As a declared return is:
  the declaration is the contract, and the contents' further precision is
  incidental, so a body dispatches on what its parameters declare. A parameter
  declared `Any` keeps its argument's type, since `Any` has no member of a
  value's kind. A module argument is viewed at its parameter's signature, as
  `:!` views it; that view door is [modules](../rewrite/modules.md)'s.
- *A tail chain returns at the outermost contract — decided.* A tail hop hands
  the frame it reaches the contract it owes, since the caller asked for the
  outermost callee's return. The innermost callee's own return is still what the
  value is checked against.
- *A data node retypes as a plain value — decided.* A knot cannot restamp its
  node, so the retype lays the node's top down as a plain container over its
  resolved cells: O(width), paid once per value per target. A retyped cyclic
  value prints its cycle one level down.
- *A retype reads a union member by member — decided.* A retype's target is
  fixed by the value and `T` alone, never by member order, and lies between the
  value's type and `T`. A `T` of the value's kind is the one-member case, and a
  tagged value's union the case where only its constructor's member qualifies.
  It costs a pass over `T`'s members with a memoized `Subtype` verdict each; a
  meet runs only where two unordered members of the value's kind both admit it,
  and `retyped` takes a scratch allocator for it.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Calls solved from their static types](static-solutions.md) — an ascribed
  argument's static type.
