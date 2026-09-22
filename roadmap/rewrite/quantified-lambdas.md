# Quantified lambdas

A function type that binds a `FOR ALL` group, and the lambda form that mints one.

**Problem.** A `FOR ALL` group binds only an expression shape:
[`ExpressionShape`](../../src/type_lattice/node.rs) carries `quantifiers` and
`bounds`, `KFunction` carries a params record and a return and nothing else, and
the walk's binder predicate ([`is_shape`](../../src/type_lattice/walk/unary.rs))
and the instantiation door
([`instantiate_quantified`](../../src/type_lattice/substitute.rs)) both match a
shape alone. So a name bound by `LET id = FN EXPR FOR ALL (Elt) (ID x :Elt) -> Elt`
has no function type to hold: [`callable_type`](../../src/elaborate/signature.rs)
hands every combined form its shape type instead, which
[dispatch](dispatch.md) requires it not to. The parser's `FOR ALL` head
([`BUILTIN_SHAPES`](../../src/parse/builtin_shapes.rs)) follows `EXPR` only, so
no lambda and no type expression spells a quantified function.

**Acceptance criteria.**

- `KFunction` binds a quantifier group in canonical form: survivors renumbered by
  first occurrence over the params record in its canonical name order, then the
  return, with a lone covariant occurrence replaced by `Never` and a lone
  contravariant one by its bound, so two quantified function types
  alpha-equivalent under a renaming intern to one node and the digest feeds the
  arity.
- A quantified `KFunction` is a binder to every walk: `shape_depth`,
  `substitute_quantified`, `instantiate_quantified`, `erase_quantified` and the
  `quantified` intern flag treat it as `ExpressionShape` is treated.
- The order relates two function types by name-paired admission under a
  collector, as `admits_shape` relates two shapes: params contravariant, return
  covariant, then `solve`. `shape_specificity` is unchanged, since a function
  type ranks in no bucket.
- The property tests' generators mint quantified function types, so the order's
  laws cover them.
- `FN FOR ALL (names) :{fields} -> Ret = (body)` is a lambda form, and
  `:(FN FOR ALL (names) :{fields} -> Ret)` its type expression; both elaborate
  through the same quantifier-group resolution an `EXPR FOR ALL` head does.
- `callable_type` hands a combined form — `LET name = FN EXPR …` and
  `LET name = FN EXPR FOR ALL …` alike — the function type over its head's slot
  names and its return, quantified where the form carries a group; only its
  bucket registration carries the shape.
- A call through the name binds its value parameters by name exactly as an
  unquantified call does, solves the callee's group against the arguments'
  carried types, and binds each type parameter to its solution; a group the
  arguments cannot solve refuses the call. A call whose return is quantified
  places as `Shares`.
- `LET name = FN FOR ALL … = (body)` unparenthesized is a reserved key, as
  `LET name = FN … = (body)` is: a diagnosable miss whose body stays raw.
- The tutorial reference lists the lambda form and its type.

**Directions.**

- *Where the group lives — decided.* On `KFunction` itself, beside `params` and
  `ret`, as `ExpressionShape` carries its own; a wrapper node would put the binder
  a level above the record it binds and break the `shape_depth` accounting.
- *One canonicalizer — decided.* `shape_type`'s census, survivor selection and
  substitution factor into one routine over `(type, variance)` pairs that both
  interning doors call.
- *A monomorphic function type binds nothing — decided.* Only a `KFunction`
  carrying a group is a binder; an unquantified `FN` type inside a `FOR ALL`
  head keeps reading the head's variables, as the elaborator opens no group for
  it. A shape is a binder even when its group is empty.
- *Where the quantifier map lives — decided.* `callable_type` hands back the
  declaration-order → canonical map beside the handle, the `Function` node
  stores it in its region and a copy re-homes it, and the frame binds the
  `k`-th type-parameter slot to the solution at `map[k]`, or to the variable's
  bound where canonical form dropped it.

## Dependencies

**Requires:** none — the lattice and the elaborator ship.

**Unblocks:**

- [Dispatch](dispatch.md) — the type a quantified combined form binds to its name.
