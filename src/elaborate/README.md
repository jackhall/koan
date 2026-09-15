# Elaborate

Type expressions turned into [type lattice](../type_lattice/README.md) handles,
read where they are written. `elaborate` sits above
[`scope`](../scope/README.md) and below [`function`](../function/README.md):
a function's type is elaborated from its signature where the function is born,
and nothing below `scope` can read a name.

## What a type expression is

A type expression is syntax in program storage — a bare type name, or a
parenthesized or sigiled group of parts. Its type names are not searched for:
the shape builder already resolved each one to a coordinate and recorded it as
a mention ([Resolution](../scope/README.md#resolution)), so
[`type_expression`](expression.rs) looks the mention up by the name part's site
through `Shape::mention` and reads it through the activation the expression is
read in. A name bound to a type value elaborates to that value's handle. A
parameter and return type of a callable are eager mentions of the enclosing
shape, so the activation a signature is read through is the one the callable
is born in.

Every composite is built from the handles its parts elaborate to, through the
registry's own doors:

- a bare name, `Number`, is the handle its binding holds;
- `LIST OF T` and `MAP K -> V` are the list and dict nodes;
- `A | B | …` is the canonical union of its members;
- `:{x :T, …}` is the record type of its fields in written order;
- `FN :{x :T, …} -> R` is the function type over the schema's fields and the
  return;
- `EXPR (head) -> R`, with or without `FOR ALL (names)`, is the expression
  shape over the head's keywords and typed slots and the return;
- `Union.Tag` is the member of the union whose tag it names.

**Quantifiers are positions, not mentions.** A name a `FOR ALL` group declares
is that group's quantifier at its written position, elaborated as
`quantified(index, Any)`, so two heads that differ only in what they call their
quantifiers intern to one shape. Groups nest: an `EXPR` type inside a signature
opens its own group, and a name the innermost group declares shadows the rest.
A name only an *outer* group declares, read under a nested group, is refused.

## A callable's type

[`callable_type`](signature.rs) reads a callable's type off the form node its
body sits in — `Shape::form` of the body shape the binder births — walking the
form's parts by their [roles](../scope/roles.rs):

- a `FN` is the function type over its `:{…}` schema and its return;
- an `EXPR` is the expression shape over its head and its return, quantified
  over its `FOR ALL` names;
- a binary `OP` is the shape `operand <symbol> operand`, returning its declared
  result, or its operand when it declares none, since a chain of it folds;
- a `UNARY OP` is the shape `<symbol> operands`, whose slot is a list of its
  operand, since its body's one parameter `operands` takes the whole run.

A module body has no callable type here.

## Refusals

A type expression that does not elaborate is an [`Elaboration`](../elaborate.rs),
never a panic and never a guess:

- `Pending` — a type name whose binder is still running, with the binder's cell
  handle, so the caller turns it into a dependency on that binder;
- `NotAType` — a type name bound to something other than a type value;
- `NoSuchMember` — a union projection naming a tag the union does not declare;
- `Unsupported` — any other spelling: constructor application
  (`Pair {Key = Number}`, `Number AS Wrap`), a `_` field, and an outer
  quantifier read under a nested group.

Elaboration writes nothing to a region: its transient runs live in the scratch
arena it is handed, and every node it builds is interned in the registry.

## The import rule

**Outside doc comments and `#[cfg(test)]`, `elaborate` names `crate::memory`,
`crate::parse`, `crate::scope`, `crate::type_lattice` and `crate::values`, and
nothing else in the crate.** It reads the form role table and the pair reader
`scope` exposes to the crate, so a type expression's parts are walked by the
same facts the shape builder walked them by.
[`tests::boundary`](tests/boundary.rs) reads this module's own source and fails
on any other `crate::` path, on an owning heap type outside the tests, and on a
retired lifetime name.

## Testing

[`tests/examples.rs`](tests/examples.rs) elaborates each production, each
refusal, and a callable's type off each form that births one, over a program
shaped and activated in a cell with every slot bound or claimed as the test
asks.

## Open work

- [Modules](../../roadmap/rewrite/modules.md) — constructor-application type
  expressions.
