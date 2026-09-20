# Operator groups

`GROUP`, a signature's operator channel, and the operator group a run of
operators chains under — each run rewritten once, where its body's shape is
built.

**Problem.** The old runtime rewrites a run of operators every time the run
dispatches: the [chain reducer](../../old_design/expressions-and-parsing.md)
probes a per-scope operator registry when the run is met, and
[`OP` and `GROUP`](../../old_design/operators.md) populate that registry as
their statements run, so how a run chains is a run-time fact an inner scope may
override. Nothing on the rewritten stack stands for any of it. The
[builtin shape table](../../src/parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact)
carries the four `GROUP` definitions and the four bodyless `GROUP` heads, and
the lattice's [`SchemaDraft`](../../src/type_lattice/schema.rs) carries an
operator channel, and no koan program reaches either: the
[type-declaration door](../../src/elaborate/README.md#declarations) elaborates a
`SIG` body's `OP` and `UNARY OP` heads as keyworded members and refuses a
bodyless `GROUP` [`Unsupported`](../../src/elaborate.rs), so a signature
declares an operator's bucket and never how a run of it chains. The parser
classifies a run as an operator chain and nothing reads the classification: a
[`BodyShape`](../../src/scope/README.md#three-tiers) holds no operator group,
so a run has nothing to chain under.

**Acceptance criteria.**

- An operator group — a set of operator symbols and the mode a run of them
  chains under: fold-left, fold-right, unary, or pairwise with a combiner symbol
  and the direction the pair results fold in — is a static fact, declared where
  it is written and nowhere else: by a `GROUP` statement, over the top-level
  `OP` statements of its unevaluated body and under its one declared mode; by a
  bare `OP` or `UNARY OP` whose symbol no group covers, as the fold-left or
  unary singleton its surface implies; by a `SIG` body's bodyless `GROUP` or
  bare operator head; and by the builtin groups. A `BodyShape` holds the groups
  its body declares in a channel of their own, and a `USING … SCOPE` body's
  shape holds the groups its operand surfaces, read off the same declaration
  [its names are](../../src/scope/README.md#names-that-arrive-at-run-time).
- A run of operators is rewritten once, where the body shape holding it is
  built, under the one group covering every symbol in it. A fold run becomes
  nested binary keyworded nodes, a unary run one keyword-first call over a list
  literal, and a pairwise run its adjacent pairs folded through the group's
  combiner written infix, every operand evaluating once.
- A quoted run is data and is not rewritten. A run in the code an `EVAL` runs is
  rewritten where that code's block shape is built, under the groups visible at
  the `EVAL`'s own position, found by a walk of the enclosing shapes that reads
  no activation; an `OP` or `GROUP` in that code chains the runs of that code
  only.
- Nothing past the body-shape builder meets a run: dispatch and the scheduler
  know no operator group, nothing probes a registry at run time, and the group
  module a `GROUP` statement binds carries no operator group.
- A symbol chains one way. A symbol has at most one operator-group declaration
  across a body shape and every shape it encloses, wherever in them the
  declarations sit; a builtin group is never redeclared; a `GROUP` naming a
  symbol a group already covers is refused; a bare `OP` on a covered symbol
  declares no group, and the overload it adds is [dispatch](dispatch.md)'s; and
  a `USING … SCOPE` whose operand surfaces a group over a symbol already covered
  where it is written is refused, while the overloads it surfaces arrive under
  the ordinary scope rules.
- A run no group covers, and a run whose symbols two groups cover, are shape
  errors, refused where the shape is built — for evaluated code, when the `EVAL`
  runs.
- A binary `OP` declaring a result type of its own is admitted wherever its
  symbol's group is pairwise, the builtin comparison group included, and refused
  elsewhere.
- A `SIG` body's bodyless `GROUP` declares one operator group over its heads,
  and a bare operator head the singleton a bare `OP` would, in the signature's
  operator channel; two signatures differing only in how their operators chain
  are two types; and a declared group contradicting a builtin group is refused
  at the `SIG`.
- Each is exercised over a program shaped and activated in a cell: a fold group,
  a pairwise group with an operand that must evaluate once, a signature
  declaring each, an `EVAL` of a quoted run, and each refusal.

**Directions.**

- *When a run is rewritten — decided.* Once per expression, where the body shape
  holding it is built, never per dispatch.
- *Where the rewrite sits — decided.* In the body-shape builder, so an operator
  group resolves where names do and nothing downstream learns of one. The
  builder constructs each rewritten node through
  [`parse`](../../src/parse/README.md)'s node constructor and walks the
  rewritten node, never the run.
- *Overriding — decided.* No group overrides another, builtin or user, so which
  group a run chains under never depends on position; position decides only
  whether a run sees a group at all.
- *The pairwise shared operand — open.* Either each operand that is not a name
  or a literal is hoisted into an anonymous slot of a synthesized block shape,
  which stays a rewrite into existing shapes and needs slots no source text can
  spell; or a pairwise node of its own carries each operand once, which the
  scheduler would stage, and so would know of. Recommended: the anonymous slots.
- *How the evaluator reaches a rewritten node — open.* Either the shape owns a
  rewritten body and nothing downstream holds the run, or a table on the shape
  maps the run's part address to its rewrite. A mention and a nested shape are
  [keyed by part address](../../src/scope/README.md#three-tiers), so a rewritten
  node exists before it is walked.
- *Which groups a run sees — open.* A run sees a group under the predicate
  [dispatch](dispatch.md) admits the group's overloads by — otherwise a run
  could chain under a group whose overloads it cannot reach — and that
  predicate is not yet fixed.
- *One group surfaced twice — open.* Two instantiations of one functor each
  surface the group their ascription declares, so nested `USING … SCOPE` bodies
  over both meet the same group twice. Whether the second is a refusal or
  changes nothing is undecided.

## Dependencies

**Requires:** none — the module value a `GROUP` binds ships.

**Unblocks:** none.
