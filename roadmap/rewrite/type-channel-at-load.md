# Elaborating the type channel at load

Type expressions turned into handles where the shape is built, not where they
run.

**Problem.** The [scope builder](../../src/scope/README.md#resolution) resolves
every type name to a coordinate and elaborates nothing: a type expression
becomes a handle only where it is read, through the activation that binds its
names ([`type_expression`](../../src/elaborate/expression.rs)), a declared
type's handle exists only once its declaration's unit has run, and a callable's
signature is elaborated afresh at every birth
([`staged`](../../src/knot/function.rs)). The one reader that answers before a
run is [`BuiltinsOnly`](../../src/elaborate/reads.rs), so the
[overlap check](../../src/dispatch/README.md#the-overlap-check) types only a
registration naming builtins alone. Where a shape is built, a `MATCH` guard, a
`MATCH … UNDER` clause and a written result type are syntax with no handle: two
guards compare only by spelling, a composite guard such as `:(LIST OF Number)`
interns afresh each time the `MATCH` runs, and a `MATCH`'s written result can be
compared to the enclosing callable's declared return only through the contract a
tail hands over. A `:Type` parameter is bound to `Any` rather than to its
argument, and a call by name that passes one is refused as misnaming the
callee's parameters ([`frame`](../../src/program/body.rs)).

**Acceptance criteria.**

- Every type binder whose definition names only builtins and closed type
  binders elaborates when the program loads, in unit order, through the
  [declaration door](../../src/elaborate/README.md#declarations) over a reader
  that reads no activation, and its slot binds that handle when its unit runs.
  Any other type binder is declared through the activation when its unit runs.
- Every type expression — a sigiled type in value position, a type part of a
  form that births no callable, a `MATCH … WITH` guard — every callable's
  signature and every registration's expression shape is typed where the shape
  is built, a quote's code included. One naming only builtins and closed binders
  carries its handle. One naming a type a run binds — a `FOR ALL` name, a `:Type`
  parameter, a name `USING … SCOPE` surfaces, a quote's hole, `\` mark or
  run-bound `$` name, a type binder declared when its unit runs — carries a rigid
  handle over one rigid variable per such name, beside the coordinate its value
  is read from; a `FOR ALL` name of the enclosing quantified callable is that
  callable's canonical quantifier.
- Where a type expression runs and where a callable is born, a closed handle is
  the value, and a rigid one is its handle with each variable replaced by what its
  coordinate reads.
- A type expression that reads a run-bound name under a meet, as a projection's
  owner, as an application's head, as a `NEEDING` kind or inside a nested
  `FOR ALL` group, and a signature that reads one inside its own group, elaborate
  where they are read, through the activation.
- A closed type that does not elaborate refuses the load, located at
  `path:line:col`; in a quote's code the refusal is kept, and the `EVAL` that
  runs the code reports it.
- A `MATCH … WITH` arm set whose guards type to one handle twice, however each is
  spelled, is refused where the shape is built.
- The overlap check refuses a registration whose closed, unquantified expression
  shape overlaps a builtin overload, whatever declared types its signature names.
- A `:Type` parameter binds the argument its call passes, by keyword or by name.
- The type lattice's node vocabulary and relations are unchanged.

**Directions.**

- *How a type is inferred — decided.* No metavariable and no substitution
  environment across statements: elaboration is local and bidirectional, a
  written annotation flowing down and a closed handle flowing up, and a
  quantifier's placeholder is the rigid `Quantified` the lattice already has.
  Hindley–Milner's generalization earns its keep on unannotated binders, and
  every koan parameter and return is annotated.
- *Where a load-time handle lives — decided.* On the shape, keyed by the
  expression's site as a mention is, in a write-once cell the builder lays down
  and a load pass in `elaborate` fills, since `scope` sits below `elaborate` and a
  shape seals before anything in it can be elaborated.
- *What a rigid handle is — decided.* A lattice handle over rigid variables, each
  paired with the coordinate the run reads its value from, so it serves
  load-time comparisons and becomes a run value by one substitution. A lattice
  node that stays symbolic through a meet or a projection would type the spellings
  left to the run too, at the cost of a node every relation must learn; they are
  rare, and the run still reads them.
- *What a `LET` of a type name admits — decided.* A type expression alone, which
  the declaration door already enforces; a computed right-hand side is refused
  at load.
- *A `:Type` parameter in a type expression — decided.* Nameable: at load a rigid
  variable, at run the argument its call passed.
- *A callable's birth — decided.* It reads the load-time type and elaborates
  through the activation only where the load left the signature unknown.
- *A static type per binder and static selection — decided.* Selection by static
  type is [static selection](static-selection.md), which types every value
  expression over this item's load pass.
- *A nominal binder over a run-bound type — deferred.* It is declared when its
  unit runs, since substitution never enters a sealed group; see
  [unplanned work](README.md#unplanned-work).

## Dependencies

**Requires:** none — the [scope builder](../../src/scope/README.md) and the
[elaborator](../../src/elaborate/README.md) are in place.

**Unblocks:**

- [Matching](matching.md) — guards, the union clause and the written result
  compared by handle where the shape is built.
- [Static selection](static-selection.md) — a static type for every value
  expression, over the load pass and its rigid variables.
