# A parameterized union's variant takes only the parameters its payload names

**Problem.** A parameterized `UNION` declares one family per variant, each over
**all** of the union's parameters
([families](../../src/elaborate/README.md#families); `declaration.rs` makes each
variant a family over the union's whole parameter list). So under the builtin
`UNION (Ok Error -> Result) = #{Ok: Ok, Error: Error}`, `(Result.Error 1)`
prints `:(Error {Error = Number, Ok = Never})(1)`: the variant carries an `Ok`
its payload says nothing about. That `Ok` is part of the variant's identity, so
`(Error {Error = Number, Ok = Str})` and `(Error {Error = Number, Ok = Number})`
are unrelated types: a `(Result.Error 1)` bound under
`:(Result {Ok = Str, Error = Number})` and another under
`:(Result {Ok = Number, Error = Number})` compare unequal (`==` is `false`). A
non-parameterized union's variant is its name, kind and payload alone
([a union's variants](../../src/elaborate/README.md#a-unions-variants)); a
parameterized union's variant is not. The elaborator's `param_names`
(`src/elaborate/expression.rs`) reads a union head's parameters only where every
member declares the same set, and `applied_union` applies every variant at the
same arguments.

**Acceptance criteria.**

- A parameterized union's variant is a family over the union's parameters its
  payload names, and no other: `Result.Error` takes `Error` alone, `Option.None`
  under `UNION (Elem -> Option) = #{Some: Elem, None: Null}` takes none, and
  `(Result.Error 1)` prints `:(Error {Error = Number})(1)`.
- `(Result.Error 1)` is one type whatever `Ok` a binder or slot names it under,
  so the two bindings in the Problem compare equal.
- A union's application is unchanged: `:(Result {Ok = Number, Error = Str})`
  still applies the union at both parameters and renders as it does today, each
  variant applied at the arguments its own parameters name.
- A construction through a parameterized union's variant still lies at most at
  the union applied at what the payload pins, each parameter the payload says
  nothing about at `Never`: `(Result.Error 1)` is at most
  `(Result {Error = Number, Ok = Never})`.

**Directions.**

- *Where the union's parameter list lives — decided.* A union's parameter list
  is read off its binder's declaration, not recovered from its members, since a
  parameter no payload names still belongs to the union.
- *How a variant over no parameter is declared — decided.* A variant whose
  payload uses no parameter (`Option.None`) is a plain nominal, so it is one
  handle with its standalone twin, as a non-parameterized union's variant is.

## Dependencies

This touches variant identity
([identity](../../lattice/src/types/identity.md)) and elaboration; any change
under `lattice/src/types` is a flagged exception needing an OK.

**Requires:** none.

**Unblocks:** none — a leaf.
