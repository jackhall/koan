# Parameterized unions

Type-constructor families that hold values: a union over type parameters, and
the construction rule that builds a value of one.

**Problem.** A type-constructor family declared as `NEWTYPE (Type AS Boxed)`
elaborates to a `TypeConstructor` schema with parameter names and no variants
([elaborate/declaration.rs](../../src/elaborate/declaration.rs)), so a family
states nothing about what it holds. The one construction rule,
[`construction`](../../src/values/admission.rs), refuses any head that is not a
newtype with `NotNewType`, so `Boxed (7)` builds nothing. A `UNION` takes no
type parameters, so no declaration can state a family with variants, and the
builtin `Result` has no declaration. An applied family (`ConstructorApply`) has
no clause in the lattice's [order](../../src/type_lattice/order.rs), so each
application lies below only itself.

**Acceptance criteria.**

- A `UNION` declaration takes type parameters, and its variants' payload types
  name them; the builtin `Result`, over `Ok` and `Error`, is declared this way.
- A one-parameter `NEWTYPE` family wraps a payload of its parameter's type.
- Constructing through a family solves its parameters against the payload's
  carried type: `Result.Ok 1` carries a `Result` whose `Ok` is `Number`, and
  `Boxed (7)` carries `:(Boxed {Type = Number})`.
- A payload a variant's type cannot be solved against is a construction
  refusal naming the variant and the payload's type.
- The [tie](../../src/knot/README.md#the-tie) checks a knot's tagged node over a
  family by the same rule an ordinary construction goes through.
- An applied family is ordered by its parameters, so a value built through a
  variant satisfies a declared application its parameters fit under.

**Directions.**

- *One construction rule — decided.* A family construction is an arm of
  `construction`, which the evaluator and the tie both go through
  ([src/values/README.md](../../src/values/README.md#what-a-value-is)).
- *The spelling of a parameterized union — open.* Recommended: the family
  spelling `NEWTYPE` already uses, `UNION (Ok Error AS Result) = (…)`.
- *A parameter a variant does not mention — open.* Recommended: it takes
  `Never`, and an applied family is covariant in its parameters, so
  `:(Result {Ok = Number, Error = Never})` lies below
  `:(Result {Ok = Number, Error = Str})` and a declared return re-types a
  `Result.Ok` to it.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Dispatch](dispatch.md) — a family construction is one of the constructions it evaluates.
- [Control expression shapes and errors](control-and-errors.md) — `CATCH` builds a `Result`.
