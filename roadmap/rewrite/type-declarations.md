# Type declarations

`NEWTYPE`, `UNION` and `SIG` turned into lattice handles: the door that brings a
component of type binders into being.

**Problem.** [Scopes](../../src/scope/README.md#visibility) resolve a type
declaration like any other binder — `NEWTYPE`, `UNION`, `SIG` and `TYPE` each
declare a name in the type channel, and a schema is a constructor context, so a
type naming itself or a later sibling is a deferred mention and a group of
mutually recursive declarations condenses into one deferred-only component.
Nothing turns that component into handles.
[`elaborate`](../../src/elaborate/README.md) walks *type expressions* — a bare
name, `LIST OF`, `MAP`, `FN`, `EXPR`, a union of members, a record type, a union
projection — and no schema: a declaration's [`Schema`
role](../../src/scope/roles.rs) reaches no door, and
`Shape::rhs` carries only a `LET` binder's right-hand side, so a declaration's
own part is not reachable from the shape at all. The
[lattice](../../src/type_lattice/README.md#recursive-groups-identity-is-the-scc-not-the-declaration)
already holds the machinery a group needs —
[`RecursiveGroupWindow`](../../src/type_lattice/window.rs) mints a member's
sibling, fills each member, and seals the group on its SCC digest — and koan's
suites reach it directly: [`function`'s
fixture](../../src/function/tests.rs) seals each newtype a test constructs by
hand and hands it in as a builtin, because no program can declare one.

**Acceptance criteria.**

- One door takes a component of type binders, a reader and a writer and hands
  back one `KType` per member in member order: a `NEWTYPE`'s newtype over its
  representation, a `UNION`'s canonical union over its variants, a `SIG`'s
  signature, and a `LET` of a type name's type expression.
- A `TYPE` in a `SIG`'s definition part is an abstract member of that
  signature, bare or higher-kinded, and is never a member of a component.
- A component of one acyclic type binder seals as a singleton group, and a
  component of mutually recursive declarations seals as one group on its SCC
  digest, so two declarations of the same shape in different programs intern
  equal.
- A member of a sealed group reads its fellows as siblings, so
  `NEWTYPE Ring = :{next :Ring}` and a `UNION` whose variant payload names a
  fellow declaration both elaborate without a placeholder.
- A declaration the door cannot elaborate refuses with the same
  [`Elaboration`](../../src/elaborate.rs) a type expression does, naming the
  site, and writes nothing to the registry.
- A type binder's declared part is reachable from its
  [shape](../../src/scope/README.md#visibility) by slot, as a `LET` binder's
  right-hand side already is.
- The door is exercised over a program shaped and activated in a cell: a lone
  newtype, a union, a signature with an abstract member, a ring of two declarations, and each refusal.

**Directions.**

- *The door's shape — decided.* The type channel's analogue of
  [the tie](../../src/function/README.md#the-tie): one call per component,
  members read before anything is installed, and a refusal that writes nothing.
  The value channel's tie and this door are the two ways a component of binders
  becomes values, and the layer above chooses by the channel its members are
  declared in.
- *Where the door lives — decided.* `elaborate`, beside
  [`callable_type`](../../src/elaborate/signature.rs): it reads names through a
  reader and builds lattice nodes, which is what that module is.
- *How a declaration's part is reached — decided.* A run beside `Shape::rhs`,
  keyed by slot, holds each type binder's whole declaration node, as
  `Shape::form` holds a callable's. The door reads which declaration it is and
  where its definition part sits from the node's
  [builtin shape](builtin-shapes.md), so a bare `NEWTYPE`, which has no
  definition part, needs no case of its own.
- *Generative versus structural sealing — decided.* Structural, per
  [the lattice](../../src/type_lattice/README.md#identity-a-handle-is-a-content-digest):
  generativity is opaque ascription's and an abstract member's alone, and a
  generative window holds exactly one member, so a declared group seals through
  `RecursiveGroupWindow::new` and a `NEWTYPE`'s identity is its name and its
  component's content.
- *Constructor application — deferred.* `Pair {Key = Number}` and
  `Number AS Wrap` are [`Unsupported`](../../src/elaborate.rs) in a type
  expression and stay so; [modules](modules.md) owns them.

## Dependencies

**Requires:**

- [Builtin shapes](builtin-shapes.md) — the door reads a declaration's
  definition part through its builtin shape's roles.

**Unblocks:**

- [The top level on the scheduler](top-level-on-the-scheduler.md) — a top-level
  task whose members are type binders has no other way to bind them.
