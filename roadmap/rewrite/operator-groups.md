# Operator groups

`GROUP`, a signature's operator channel, and the chain a run of operators
reduces by — rewritten in the AST where it is written.

**Problem.** The old runtime reduces a run of operators dynamically: the
[chain reducer](../../old_design/expressions-and-parsing.md) probes a per-scope
operator registry when the run is met, and
[`OP` and `GROUP`](../../old_design/operators.md) populate that registry as
their statements run. Nothing on the rewritten stack stands for any of it. The
[builtin shape table](../../src/parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact)
carries the four `GROUP` definitions and the four bodyless `GROUP` heads, and
the lattice's [`SchemaDraft`](../../src/type_lattice/schema.rs) carries an
operator channel of chaining records, and no koan program reaches either: the
[type-declaration door](../../src/elaborate/README.md#declarations) elaborates a `SIG` body's `OP`
and `UNARY OP` heads as keyworded members and refuses a bodyless `GROUP`
[`Unsupported`](../../src/elaborate.rs), so a signature declares an operator's
bucket and never how a run of it chains. A `GROUP` binds a
[module value](../../src/knot/module/README.md), whose signature carries no operator
record.

**Acceptance criteria.**

- A run of operators is rewritten statically into the calls it reduces to, by
  the chaining record in scope where the run is written, and nothing reduces a
  chain at run time.
- A `GROUP` definition binds a module value whose members chain under its one
  declared mode, and a bare `OP` or `UNARY OP` chains under the singleton record
  its surface implies.
- A `SIG` body's bodyless `GROUP` declares one chaining record over its heads,
  and a bare operator head its singleton, in the signature's operator channel;
  two signatures differing only in how their operators chain are two types.
- Each form is exercised over a program shaped and activated in a cell: a fold
  group, a pairwise group, a signature declaring each, and each refusal.

**Directions.**

- *When a chain reduces — decided.* Statically, as a rewrite of the AST, rather
  than dynamically where the run is evaluated.
- *Where the rewrite reads its chaining records — open.* The records a run
  reduces by are declared by `OP` and `GROUP` statements and surfaced by
  `USING … SCOPE`, so the rewrite needs them where names resolve. Either the
  [shape builder](../../src/scope/README.md#resolution) resolves a run's record
  as it resolves a name, or a pass of its own runs between parse and shape.

## Dependencies

**Requires:** none — the module value a `GROUP` binds ships.

**Unblocks:** none.
