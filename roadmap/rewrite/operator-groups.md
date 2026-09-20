# Operator groups

`GROUP`, a signature's operator channel, and the operator group an operator run
chains under — each operator run rewritten once, where its body's shape is
built.

**Problem.** The old runtime rewrites an operator run — a slot-led expression
of two or more operators — every time it dispatches: the
[chain reducer](../../old_design/expressions-and-parsing.md) probes a per-scope
operator registry when the operator run is met, and
[`OP` and `GROUP`](../../old_design/operators.md) populate that registry as
their statements run, so how an operator run chains is a fact of the moment it
dispatches, which an inner scope may override. Nothing on the rewritten stack
stands for any of it. The
[builtin shape table](../../src/parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact)
carries the four `GROUP` definitions and the four bodyless `GROUP` heads, and
the lattice's [`SchemaDraft`](../../src/type_lattice/schema.rs) carries an
operator channel, and no koan program reaches either: the
[type-declaration door](../../src/elaborate/README.md#declarations) elaborates a
`SIG` body's `OP` and `UNARY OP` heads as keyworded members and refuses a
bodyless `GROUP` [`Unsupported`](../../src/elaborate.rs), so a signature
declares an operator's bucket and never how an operator run of it chains. The
parser classifies an operator run and nothing reads the classification: a
[`BodyShape`](../../src/scope/README.md#three-tiers) holds no operator group and
no statements of its own, so an operator run has nothing to chain under and
nowhere to be rewritten into, and the
[elaborator](../../src/elaborate/README.md#what-a-type-expression-is) reads a
union's operator run part by part.

**Acceptance criteria.**

- An operator group — a set of operator symbols and the mode an operator run of
  them chains under: fold-left, fold-right, unary, or pairwise with a combiner
  symbol and the direction the pair results fold in — is a static fact whose
  identity is its content: two declarations of an equal member set under an
  equal mode are one group. A group is declared by a `GROUP` statement, over the
  top-level `OP` statements of its unevaluated body and under its one declared
  mode; by a `SIG` body's bodyless `GROUP`; and by the builtin groups. A bare
  `OP` declares no group, and a `UNARY OP` marks its symbol unary.
- A symbol chains one way, decided for the whole program wherever its
  declarations sit: under the builtin group covering it, else under the one
  group the program's `GROUP` statements declare over it, else as unary where a
  `UNARY OP` names it, else fold-left and alone. A `GROUP` whose members overlap
  a group it does not equal — a builtin group or another statement's — or name a
  unary symbol is refused.
- A `BodyShape` holds groups in a channel of their own: a `GROUP`'s body holds
  its group, and a `USING … SCOPE` body's shape holds the groups its operand
  surfaces, read off the same declaration
  [its names are](../../src/scope/README.md#names-that-arrive-at-run-time) — a
  `GROUP` binder's group, or the bodyless `GROUP`s of the `SIG` an ascription
  names. A `USING … SCOPE` surfacing a group that overlaps, without equalling, a
  group covering a member where it is written is refused; surfacing an equal
  group changes nothing, and the overloads it surfaces arrive under the ordinary
  scope rules.
- An operator run is rewritten once, where the body shape holding it is built,
  under the one chaining every symbol in it shares: a builtin group, a group an
  enclosing shape holds, or the unary or fold-left chaining of a lone symbol no
  group claims. A fold operator run becomes nested binary keyworded nodes, a
  unary one a keyword-first call over a list literal, and a pairwise one its
  adjacent pairs folded through the group's combiner written infix, every
  operand evaluating once. An operator run in a type expression is rewritten
  like any other.
- A body shape owns its statements, rewritten, and every part address it records
  lies inside them. Nothing past the body-shape builder meets an operator run:
  the elaborator reads a union as the rewritten call, dispatch and the scheduler
  know no operator group, nothing probes a registry at run time, and the group
  module a `GROUP` statement binds carries no operator group.
- A quoted operator run is data and is not rewritten. An operator run in the
  code an `EVAL` runs is rewritten where that code's block shape is built, under
  the groups the shapes enclosing the `EVAL` hold, found by a walk that reads no
  activation slot; a `GROUP` in that code is held to the program's declarations
  and chains the operator runs of that code only.
- `==` and `!=` belong to no group and join whichever pairwise group the rest of
  an operator run chains under, folding through that group's combiner; an
  operator run of them alone folds pairwise through `AND`, and one whose other
  symbols chain any other way is refused. A user's `==` returns `Bool`, a
  declaration naming `!=` is refused, and `a != b` is always the opposite of
  `a == b` under whichever `==` is selected.
- An operator run naming a symbol whose group is not visible where it is
  written, and one whose symbols chain under different groups, are shape errors,
  refused where the shape is built — for evaluated code, when the `EVAL` runs.
- A binary `OP` declaring a result type of its own is admitted wherever its
  symbol's group is pairwise, a builtin pairwise group included, and refused
  elsewhere.
- A `SIG` body's bodyless `GROUP` declares one operator group over its heads in
  the signature's operator channel, and a bare operator head declares its bucket
  alone; two signatures differing only in how their operators chain are two
  types; a declared group overlapping a builtin group it does not equal is
  refused at the `SIG`; and a module's
  [self-signature](../../src/elaborate/README.md#a-modules-self-signature)
  carries the groups its body's shape holds, so a `GROUP` module satisfies the
  signature declaring its group.
- Each is exercised over a program shaped and activated in a cell: a fold group,
  a pairwise group with an operand that must evaluate once, the fold-left
  chaining of a bare `OP` declared in two sibling modules, a unary operator run,
  a union type's operator run, equality chained alone and beside a pairwise group,
  a signature declaring each group, equal groups
  declared twice, an `EVAL` of a quoted operator run, and each refusal.

**Directions.**

- *When an operator run is rewritten — decided.* Once per expression, where the
  body shape holding it is built, never per dispatch.
- *Where the rewrite sits — decided.* In the body-shape builder, so an operator
  group resolves where names do and nothing downstream learns of one. The
  builder constructs each rewritten node through
  [`parse`](../../src/parse/README.md)'s node constructor and walks the
  rewritten node, never the operator run.
- *Overriding — decided.* No group overrides another, builtin or user, so which
  group an operator run chains under never depends on position; position decides
  only whether it sees a group at all.
- *What a bare `OP` declares — decided.* An overload and nothing else. A symbol
  no group claims chains fold-left alone as a builtin default, so whether an
  `OP` declares a group never depends on what is declared around it, and two
  modules overloading one symbol never conflict. A `UNARY OP` marks its symbol
  unary the same way, since the default cannot tell a unary operator run from a
  fold.
- *Group identity — decided.* Content. A group holds symbols and a mode and no
  function, so equality is the comparison the
  [lattice](../../src/type_lattice/README.md) already makes for satisfaction. A
  functor's `GROUP` is one group however often it is instantiated, and two
  signatures or two functors declaring equal groups agree.
- *The pairwise shared operand — decided.* Each operand that is not a name or a
  literal is hoisted, in source order, into an anonymous slot of a synthesized
  block shape, under a name no source text can spell. The rewrite stays inside
  existing shapes at the price of one rule for the evaluator: an expression part
  holding a nested block shape runs as a block whose value is its last
  statement's. A pairwise node of its own would be the scheduler's to stage.
- *How the evaluator reaches a rewritten node — decided.* The shape owns the
  rewritten body, copied from each operator run up to its statement and nowhere
  else, so nothing downstream holds an operator run and a statement with none
  keeps its addresses. A table from an operator run's part address to its
  rewrite would put a lookup in every reader and cannot key a statement that is
  itself an operator run.
- *Which groups an operator run sees — decided.* The groups held by the shapes
  enclosing it: a `GROUP`'s own body and a `USING … SCOPE` body, which is where
  [dispatch](dispatch.md) admits the group's overloads. Both hold a group as a
  parameter is held, so no position is compared. An operator run over a claimed
  symbol whose group it cannot see is refused rather than chained by default.
- *One group surfaced twice — decided.* An equal group changes nothing, so
  nested `USING … SCOPE` bodies over two instantiations of one functor build.
- *How equality chains — decided.* `==` and `!=` take `Any`, so they belong to
  no group and join any pairwise one, which needs no precedence between
  overlapping groups and keeps a symbol chaining one way. A member of the
  builtin comparison group could not chain beside a user's pairwise operators.
- *How `!=` stays the opposite of `==` — decided.* The builder rewrites every
  infix `a != b` as `NOT (a == b)`, so `!=` has no bucket, never reaches
  [dispatch](dispatch.md), and is opposite by construction. A closed builtin
  `!=` negating a dispatch of `==` would be a native body re-entering dispatch.

## Dependencies

**Requires:** none — the module value a `GROUP` binds ships.

**Unblocks:** none.
