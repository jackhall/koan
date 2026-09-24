# Dispatch

Keyword lookup over scopes: choosing the callable a keyworded expression runs,
and binding its arguments.

**Problem.** The old runtime's dispatch keeps its registrations in the keyed
channel of [machine/core/bindings.rs](../../src/machine/core/bindings.rs), and
[`resolve_dispatch`](../../src/machine/execute/decide/resolve_dispatch.rs)
walks the scope chain to a `Resolved`, `Ambiguous`, `ParkOnProducers`,
`UnboundName` or `Unmatched` outcome. Builtins — `LET` and `PRINT` included —
register as functions through
[`register_builtin`](../../src/builtins.rs), so even a program that declares
no function dispatches on every statement, and a reference to a binder that
has dispatched but not yet bound parks through name placeholders
([name-placeholders.md](../../old_design/execution/name-placeholders.md)). The
rewrite's [scopes](../../src/scope/README.md) resolve value and type names
only: nothing selects a callable for a keyworded expression, so no koan
program runs on the rewritten stack.

**Acceptance criteria.**

- Dispatch keys a keyworded registration on its full bucket key, and a typed
  argument that does not satisfy a candidate is a non-match that falls through
  rather than a bind-time error.
- A call runs the unique most specific admitting candidate; two admitting
  candidates that are neither ordered nor resolved by the tie-breaks are an
  ambiguity error, and no admitting candidate is a no-overload error naming the
  arguments' types.
- A keyworded use with no candidate at all — no builtin overload and no visible
  registration at its key — is refused where its shape is built, as an unbound
  name is.
- An overload is a candidate only where the scopes' visibility predicate admits
  it.
- A [`BUILTIN_SHAPES`](../../src/parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact)
  entry's bucket is closed: a user registration at its untyped key is rejected.
- A builtin operator's bucket — arithmetic, comparison, `AND`, the type union
  `|` — admits user overloads, the builtin's own overloads are selected first,
  and a user overload whose operand type is known, where the shape is built, to
  overlap a builtin's is rejected.
- A shadowable builtin's bucket — equality, whose operands are `Any` — admits a
  user overload that is selected in the builtin's place.
- A reference to a visible binder always reads it bound: the shape orders its
  binder first. A dispatch placeholder keys on the full bucket key.
- A combined expression shape — `LET f = FN EXPR …`, `LET plus = OP …` — binds
  a lambda to its name and registers its expression shape under its bucket:
  [`callable_type`](../../src/elaborate/signature.rs) already hands a named
  callable [its function type](../../src/elaborate/README.md#a-callables-type),
  quantified where the expression shape carries a `FOR ALL` group, so only a
  bucket registration carries an `ExpressionShape`.
- A bucket-only definition — a bare `EXPR` or `OP` statement — is a bound member
  of the activation it is declared in, and a module's
  [self-signature](../../src/elaborate/README.md#a-modules-self-signature)
  carries it in its keyworded channel; a combined expression shape's bucket is
  carried there beside its named value slot.
- A call in a body's tail position whose declared return satisfies the
  caller's runs as a tail hop: a tail recursion N deep holds O(1) cells.
- A frame's value satisfies its callee's declared return and carries it — a
  container retyped to the declared type, a tagged value to the union member
  naming its constructor — or the call yields an error.
- A registration whose bucket key holds no keyword, anywhere in it, is
  refused where its shape is built.
- An evaluation or a frame that cannot proceed yields a koan error value, every
  evaluation passes an error it receives through unchanged, and an uncaught one
  ends the program with `error: <message>`.
- Every runnable tutorial snippet that uses no `MATCH`, `TRY`, `CATCH`,
  `Result`, `MODULE`, `SIG`, `VAL`, `TYPE`, `USING`, `:|`, `:!` or `CLOSE` runs
  on the rewritten stack and prints the output its tutorial shows, and
  `tools/verify_snippets.py` reads the rewritten binary, skipping
  the snippets that use an expression shape on its pending list.
- The module's design doc is the `README.md` in its source directory, and the
  module's top-of-file comment links it.

**Directions.**

- *What dispatch runs — decided.* Literals, names, containers, record access
  (`.`, `ATTR`, `FROM`), newtype, type-constructor family and union-variant
  construction, functions, keyword dispatch, the builtin library, quotes and
  `$(…)`, and uncaught errors. [Control expression shapes and
  errors](control-and-errors.md) owns `MATCH`, `TRY`, `CATCH` and `Result`, and
  [modules](modules.md) owns the module expression shapes.
- *Builtins as function values — decided.* A builtin registers through the same
  bucket a user function does, as a function value whose body is native.
- *Newtype construction — decided.* An ordinary construction `(Head payload)`
  is `Tagged::construct`, the one construction rule the tie checks a knot's
  tagged nodes by too ([src/values/README.md](../../src/values/README.md#what-a-value-is)).
- *The lambda a combined operator or a quantified combined expression shape binds —
  decided.* A binary `OP` body's parameters are `left` and `right`, so
  `LET plus = OP #(⊕) OVER Number` binds `FN :{left :Number, right :Number} ->
  Number`; a unary one's is `operands`, so it binds
  `FN :{operands :(LIST OF Number)} -> Number`. A `FN EXPR FOR ALL (Elem) …`
  binds the quantified function type `callable_type` builds for it.
  Quantification changes nothing about how a call by name binds its frame:
  arguments bind by name, the frame
  [solves the group](../../src/program/README.md#the-body-runner) against them,
  and a typed argument the callee's group cannot be solved against is the
  ordinary type mismatch.
- *Keyword reads resolved in the shape — decided.* A bucket key resolves when
  the shape is built, as a value or type name does
  ([src/scope/README.md](../../src/scope/README.md#resolution)), to a
  **candidate list**: a fixed run of coordinates — the builtin's own overloads
  read off the builtin table, then the registrations at that key visible to the
  use in each enclosing scope, outermost first, filtered by the visibility
  predicate. A keyworded use is a **mention of each candidate**, classified
  eager or deferred as a name mention is: at a statement it reads at the
  statement's position, so a registration declared after it is invisible, as a
  function called before its `LET` is; inside a callable body it is deferred, so
  the body sees its own registration and later ones, captures them, and
  mutually recursive registrations tie as one knot. A candidate's slot types are
  elaborated when its function is born, so no order is fixed where the shape is
  built; the per-call residue is the type test and the selection rule below. A
  keyworded use inside a quote resolves where the quote is written, as one in a
  callable body does ([quotes resolve where they are written](eval-scope.md));
  a registration in evaluated code is a member of the block shape it runs in, a
  candidate for the statements after it there, and never widens a bucket around
  it, so no candidate list computed for a static site changes after it is
  built. The closed-bucket and overlap checks run where a shape is built, so
  code composed at run time fails them when it is evaluated.
- *Builtin buckets a user adds to — decided.* A `BUILTIN_SHAPES` entry is closed
  at its key because the body-shape builder walks a matching node by the entry's
  roles. An operator's bucket is open by type because its slots are eager
  operands whichever overload is selected, so no walk depends on the selection.
  How an operator run of it chains is the
  [shape builder](../../src/scope/README.md#operator-groups)'s and is never a
  user's to change.
- *Shadowable builtins — decided.* One selection rule serves every open
  bucket: the most specific candidate wins, and a builtin wins a tie. A user
  overload that overlaps a builtin's operand type is rejected where the shape
  is built, except under a builtin whose operands are `Any`, which admits the
  overlap — refusing it would leave the builtin un-overloadable — and is then
  beaten by any user overload, since every type is more specific than `Any`.
  The shadowable kind is therefore derived, not listed: a builtin is shadowable
  exactly where its operands are `Any`. A user's `==` returns `Bool` and `!=`
  is never declared: the [shape builder](../../src/scope/README.md#operator-groups)
  holds a program to both and rewrites `a != b` as `NOT (a == b)`, so `NOT` is
  a builtin over `Bool` and `!=` has no bucket. `PRINT`'s operand is `Any`, so
  a user overload of it is admitted and wins for its type; `PRINT` is a
  stopgap until koan has effects.
- *Selection — decided.* Each call keeps the candidates whose expression shape
  admits the arguments' carried types and runs the unique most specific one,
  ranked by the type lattice's shape order
  ([`shape_specificity`](../../src/type_lattice/sig_relations.rs)), the order a
  signature's bucket replay ranks by too; so a quantified candidate ranks below
  a concrete one that admits the same arguments, and dispatch compares no slot
  types of its own. Where several are maximal, a builtin among them wins;
  otherwise equally specific candidates from different scopes resolve to the
  outermost; anything else is an ambiguity error. The overlap check runs at load over the
  operand types spelled from builtin names alone, so an operand type known only
  at birth is never checked.
- *Errors — decided.* A koan error is a tagged value of the builtin nominal
  `Error` over `{message :Str}`, and a consumer checks the results it reads, per
  the [scheduler](../../src/scheduler/README.md#the-drain). The program stops at
  the first uncaught error. [Control expression shapes and
  errors](control-and-errors.md) widens the payload when `CATCH` needs more.
- *Tail calls — decided.* A frame evaluates its last statement in its own cell,
  with its declared return as the contract; when that statement's selected call
  returns a type satisfying the contract, the cell hops to the callee's frame
  instead of spawning it, and otherwise it checks the value on finish.
- *What the tutorial shows — decided.* A snippet whose output the rewrite
  changes — an error's text, or a behaviour the rewrite's design changed — is
  rewritten to the rewrite's output. `tools/verify_snippets.py` skips a snippet
  that uses an expression shape on a pending list, which later items shrink.
- *An overload that is never selected — decided.* A functor is a `FN` or
  `EXPR` returning a module, so an `OP #(+) OVER Elt` in its body learns its
  operand type per call: at `Elt = Number` the builtin is selected first,
  inside the functor's own body too. Two user overloads meet the same way when
  an enclosing scope already holds the instantiated one. Nothing reports it:
  koan has no warning channel, and the case is recorded as the motivation for
  one under [unplanned work](README.md#unplanned-work).
- *Visibility shared with operator groups — decided.* An operator run sees a
  user's [operator group](../../src/scope/README.md#operator-groups) inside the
  `GROUP`'s own body and
  inside a `USING … SCOPE` body surfacing it, which is where the group's
  overloads are admitted here; builtin chaining is seen everywhere.
- *What dispatch evaluates — decided.* The statements a body's shape owns,
  [rewritten](../../src/scope/README.md#operator-groups), never the parse; and an
  expression part holding a nested block shape runs as a block whose value is
  its last statement's, which is how a pairwise operator run's shared operand
  evaluates once.

## Dependencies

**Requires:**

- [Code as values](code-values.md) — `ATTR`'s symbol label, `EVAL`'s operand, and code-typed parameters.
- [Parameterized unions](parameterized-unions.md) — the family construction rule.
- [Binders nested in expressions](nested-binders.md) — a nested binder is hoisted before dispatch meets it.
- [Callables typed by function types](function-typed-callables.md) — each candidate's shape, and one group solve.
- [Lambdas born where they are written](lambdas-where-written.md) — the door the evaluator births a lambda through.
- [Record field types in type position](record-field-types.md) — tutorial 08's `LABEL` snippet.
- [Quotes resolve where they are written](eval-scope.md) — a quote's names and candidates resolve at the quote.

**Unblocks:**

- [Modules](modules.md) — a module program runs only under dispatch.
- [Yielding iterators](yielding-iterators.md) — a demand for an element is an ordinary dispatch.
- [A compact type node table](compact-type-node-table.md) — its presize is calibrated on running programs.
- [Control expression shapes and errors](control-and-errors.md) — arms run as blocks, and errors are values, here.
- [Slicing and splicing](slicing-and-splicing.md) — slicing and splicing are builtins.
