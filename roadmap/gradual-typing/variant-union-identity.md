# A construction through a union binder types at its union

**Problem.** A union variant construction is at most `Any`
([static types](../../src/dispatch/README.md#static-types)), so the load knows
nothing of `(Maybe.Some 3)` and a call reads its carried type, the variant
`Some`, never its union. A `MATCH … UNDER` clause over a `FOR ALL` name such a
call binds reads a type that is no union: under
`EXPR FOR ALL #[Un] #(PICK x :Un) -> Any = #(MATCH x UNDER Un WITH #{Some: (it), _: (0)})`,
`PICK (Maybe.Some 3)` is refused as `` `MATCH … UNDER` takes a union, and Some
is not one; a lone variant is not its union ``
([a union's variants](../../src/elaborate/README.md#a-unions-variants)), while
`PICK m` over `LET m :Maybe = (Maybe.Some 3)` runs. The user spelled `Maybe`
once already; being made to repeat it in `LET m :Maybe = (Maybe.Some 3)` is
surprising.

**Acceptance criteria.**

- A construction through a union binder is at most the union the path reaches
  the variant through: `(Maybe.Some 3)` is at most `Maybe`, so
  `PICK (Maybe.Some 3)` over the `PICK` above prints `3`, and
  `LET x :Maybe = (Maybe.Some 3)` and `LET x :(Maybe | Number) = (Maybe.Some 3)`
  are *always* at load.
- A construction through a parameterized union's variant is at most the union
  applied at what the payload's static upper end pins, each parameter the
  payload says nothing about at `Never`: `(Option.Some 3)` is at most
  `(Number -> Option)` and `(Tree.Leaf null)` at most `(Never -> Tree)`.
- A construction through a `NEWTYPE` is unchanged: with `NEWTYPE Some = Number`,
  `(Some 4)` is `Some`.
- A tag projection reads through a variant's payload as a field projection
  does: under `UNION Shape = #{Sized: Maybe, Flat: Null}`, `Shape.Sized.Some`
  names `Maybe`'s `Some`, and `(Shape.Sized.Some 3)` is at most `Maybe`, the
  union the path reaches the variant through — it builds no `Sized`.
- The [static types](../../src/dispatch/README.md#static-types) construction
  bullet states the rule.

**Directions.**

- *Identity is unchanged — decided.* A variant's identity stays its name, kind
  and payload ([identity](../../lattice/src/types/identity.md#recursive-groups-identity-is-the-scc-not-the-declaration)):
  `Maybe.Some`, `Opt.Some` and a `NEWTYPE Some` are one handle, and
  `LET a :Opt = (Maybe.Some 3)` stays a *maybe* the run admits. The union a
  construction is reached through is a fact of the path spelled, read by the
  load, not of the type.

## Dependencies

**Requires:** none — union variants, families and `MATCH … UNDER` have shipped.

**Unblocks:** none — a leaf.
