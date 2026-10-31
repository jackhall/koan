# A variant's union in its identity

**Problem.** A variant's identity is its name, kind and payload alone: a
non-recursive member declared inside a group unifies with its standalone twin
([identity](../../lattice/src/types/identity.md#recursive-groups-identity-is-the-scc-not-the-declaration)).
So under `UNION Maybe = #{Some: Number, None: Null}`,
`UNION Opt = #{Some: Number, Nil: Null}` and `NEWTYPE Some = Number`,
`Maybe.Some`, `Opt.Some` and the newtype `Some` are one type:
`LET a :Opt = (Maybe.Some 3)` and `LET b :Maybe = (Some 4)` both load and run.
A value built by a variant's constructor carries the variant, never its union,
so a `MATCH … UNDER` clause over a `FOR ALL` name that a call binds to such a
value reads a type that is no union: under
`EXPR FOR ALL #[Un] #(PICK x :Un) -> Any = #(MATCH x UNDER Un WITH #{Some: (it), _: (0)})`,
`PICK (Maybe.Some 3)` is refused as `` `MATCH … UNDER` takes a union, and Some
is not one; a lone variant is not its union ``
([a union's variants](../../src/elaborate/README.md#a-unions-variants)), while
`PICK m` over `LET m :Maybe = (Maybe.Some 3)` runs.

**Acceptance criteria.**

- A union variant's type identity includes its declaring union: the binder and
  its sibling variants. `Maybe.Some`, `Opt.Some` and a `NEWTYPE Some = Number`
  are three distinct types.
- `LET a :Opt = (Maybe.Some 3)` and `LET b :Maybe = (Some 4)`, with
  `NEWTYPE Some = Number`, are each refused.
- A `MATCH … UNDER` clause reads a variant as its declaring union, so
  `PICK (Maybe.Some 3)` over the `PICK` above prints `3`.
- The lattice's identity design states the rule for a union's variants in place
  of the standalone-twin rule.

**Directions.**

- *Whether to make the change — open.* It changes the
  [type lattice](../../lattice/src/types/README.md)'s identity rule, which needs
  the user's approval before work starts; the alternative is to keep the
  refusal of a clause bound to a lone variant.
- *What a variant's digest folds in — open.* The declaring binder's name alone,
  or the binder and each sibling's name, kind and payload, which a group's
  seal already presents. Recommended: the binder and its siblings, as the
  ruling states it.

## Dependencies

**Requires:** none — union variants and `MATCH … UNDER` have shipped.

**Unblocks:** none — a leaf.
