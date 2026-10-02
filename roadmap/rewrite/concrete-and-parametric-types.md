# Concrete and parametric types

Concrete and parametric types are two Rust types. The lattice's order, join and
meet relate only concrete types.

**Problem.** One Rust type, [`KType`](../../src/type_lattice/handle.rs), names
every koan type. That includes:

- a concrete type;
- a quantified callable's type, and a position inside one that reads its
  `FOR ALL` group;
- a signature member's type over a head parameter;
- a load-time type over a lexical variable.

The compiler cannot tell these apart, so the code tells them apart at run time
or by convention:

- Every [registry](../../src/type_lattice/registry.rs) door that builds a type
  from child types accepts a quantified callable's type as a child: `list`,
  `dict`, `record`, `union_of`, `constructor_apply`, a function type's
  parameters, and an application's pins. `join` builds a union of two such
  types.
- Call-only is enforced only by the
  [scope builder](../../src/scope/shape/build.rs)'s `QuantifiedLambda` and
  `QuantifiedRead` refusals.
- The [order](../../src/type_lattice/order.rs) relates rigid variables and
  compares quantified callables by handle. The
  [property generators](../../src/type_lattice/tests/generators.rs) nest
  quantified types under lists, unions and function types, so the lattice's laws
  are checked over types no program should build.
- The [type channel](../../src/elaborate/channel.rs)'s `FREE` debug asserts
  check that no free `Quantified` escaped a binder door.
- [`type_satisfies`](../../src/values/admission.rs) branches on whether its slot
  holds a free `Quantified`.
- Dispatch's [selection](../../src/dispatch/select.rs) and
  [evaluation](../../src/dispatch/evaluate.rs), the
  [overlap check](../../src/dispatch/check.rs) and the
  [frame](../../src/program/body.rs) each re-test whether a callable's group is
  empty.
- `Callable` and `Registered` in [`scope/typed.rs`](../../src/scope/typed.rs)
  each hold a `quantifier_map`. The two maps share one Rust type but index
  different groups.
- The registry's `parameter`, `quantified` and `lexical` doors check that a
  bound is variable-free with debug asserts.

**Acceptance criteria.**

- `KType` names a concrete type: outside a sealed `Signature` or `SetMember`
  node, it holds no `FOR ALL` group, no free `Quantified`, no head `Parameter`
  and no `Lexical`. `Parametric` names a type that may hold such a variable,
  `Scheme` names a quantified callable's type, and `DeclaredType` is either a
  type or a `Scheme`. A `KType` converts into a `Parametric`. A `Parametric`
  becomes a `KType` only by substitution, or through a checked conversion that
  refuses a type holding a variable.
- Every door that builds a type from child types yields a `KType` from `KType`
  children. Handing it a parametric child where a `KType` is wanted is a compile
  error, pinned by a compile-fail test.
- A signature's schema takes a quantified type for a keyworded member and for a
  `VAL` member through a door of its own, and the signature is concrete.
- `is_subtype_of`, `join` and `meet` take and return `KType`s, and `union_of`
  returns a `KType` from `KType` members. Their laws hold by handle over every
  generated concrete type. The generators build no parametric type into a
  concrete one.
- A union holding a variable keeps it beside every concrete member, even a
  member its bound lies under. Only the concrete members are reduced by the
  order.
- *Fits*, the unifier, and the ranking and judging relations are the only
  relations that read a parametric type. Over concrete types *fits* contains the
  order. It is reflexive and transitive over every generated type, parametric
  ones included.
- The registry's `parameter`, `quantified` and `lexical` doors, and
  `Collector`, take each bound as a `KType`.
- `Value::ktype` answers a `DeclaredType`: a `KType` for every value except a
  quantified callable, which answers its `Scheme`.
- A key's candidate capture is laid down typed `List<Any>`, without reading its
  functions' types, and may hold a quantified registration beside a concrete
  one. The type of every other koan `List`, `Dict` or `Record` value is a
  `KType` joined from its elements' types.
- `EXPR FOR ALL #[Elt] #(PICK x :(Elt & Number)) -> Elt = #(x)` refuses the load
  where `&` is written, and so does a meet over a signature's head parameter in a
  member type.
- An expression shape with no `FOR ALL` group binds nothing: a free variable
  inside one reads the enclosing group, as it does inside a function type with
  no group.
- `Static::Closed` holds a `KType` and `Static::Rigid` a parametric type, and
  so do the static types dispatch fixes at load.
- Whether a callable is quantified is read off its type's Rust variant.
  `contains_quantified` and `contains_rigid` are private to the lattice.
- `Callable` and `Registered` each carry their quantifier map in a type of its
  own.
- Every type's digest is unchanged, as the
  [golden digests](../../src/type_lattice/tests/golden.rs) pin.

**Directions.**

- *Concrete means closed up to sealed nodes — decided.* A `Signature` is
  concrete although its schema holds head parameters and member groups. A
  `SetMember` is concrete although its family's representation reads the
  family's quantifiers. No walk, probe or substitution enters either.
- *What is parametric — decided.* Parametric types are:
  - a `FOR ALL` variable, whether a free `Quantified` or read at load as a
    `Lexical`;
  - a signature's head parameter;
  - a quantified callable's type.

  An opaque `:|` carrier is concrete, since values carry it and dispatch on it.
  A carrier as a variable's bound stays refused until [modules](modules.md)
  re-keys carriers.
- *A shape with no `FOR ALL` is concrete — decided.* A function type with no
  group already is. Neither binds anything, so neither has a free variable to
  capture.
- *The lattice's relations — decided.* The order, `join`, `meet` and union
  canonicalization relate concrete types and nothing else, so their laws are the
  concrete lattice's laws. The solving relations relate parametric types, and
  the solve is rewritten wherever it reads the order over a variable.
- *Lists the interpreter holds — decided.* A registration's bucket and a key's
  candidate capture may hold either kind of handle, since no name reaches them.
  A container a program can read holds only concrete types. A quantified
  callable is made concrete before it is stored there
  ([instantiating a quantified function](../gradual-typing/instantiating-quantified-functions.md)).
  A key's candidate list is laid down through a door of its own, typed
  `List<Any>`; dispatch reads each function's own type, and the load reads the
  list as unknown.
- *Two parametric Rust types — decided.* A `Scheme` names a quantified
  callable's type, and `Parametric` names every other parametric type. A
  quantified callable's type is the only parametric type a value carries and a
  call instantiates. So a `Scheme`'s accessors hand out its positions as
  `Parametric`.
- *Names — decided.* `KType` is concrete, `Parametric` may hold a variable,
  `Scheme` is a quantified callable's type, and `DeclaredType` is either a type
  or a `Scheme`. Callables, registered shapes, signature members and a function
  value's type are `DeclaredType`s.
- *The rigid rule belongs to* fits *— decided.* A variable lies under its bound,
  and above only itself, `Never` and its lower end. The relation is not read
  through the variables' ends, since that would lose verdicts such as `Elt`
  lying under `Elt`. A union holding a variable is canonicalized by the order
  over its concrete members alone.
- *The solver's meet — decided.* Where a solve meets two parametric types — a
  variable's least instance over upper contributions alone, or the value slots
  *fits* pools from an offered signature — it uses a meet private to the
  lattice, which relates a variable by the rigid rule. No verdict a solve
  reaches changes. The public `meet` takes concrete types.
- *A meet over a variable — decided.* `&` whose operand holds a `FOR ALL`
  variable or a head parameter is refused where it is written. Each call solves
  the variable from the slot's type, so the meet cannot wait until the variable
  is known, and no intersection type exists to keep it symbolic.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Instantiating a quantified function](../gradual-typing/instantiating-quantified-functions.md)
  — an instance is the conversion from a quantified callable's type to a `KType`.
