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
  binder first, and rejects a body where an `EVAL` and a binder declared before
  it read each other. A dispatch placeholder keys on the full bucket key.
- A combined form — `LET f = FN EXPR …`, `LET plus = OP …` — binds a lambda to
  its name and registers its expression shape under its bucket:
  [`callable_type`](../../src/elaborate/signature.rs) already hands a named
  callable [its function type](../../src/elaborate/README.md#a-callables-type),
  quantified where the form carries a `FOR ALL` group, so only a bucket
  registration carries an `ExpressionShape`.
- A statement containing `EVAL` at any nesting depth, a callable body on its
  right-hand side included, follows every unit binding a name or a bucket
  registration declared before its position; the `EvalCycle` refusal covers a
  binder that waits on such a statement.
- A bucket-only definition — a bare `EXPR` or `OP` statement — is a bound member
  of the activation it is declared in, and a module's
  [self-signature](../../src/elaborate/README.md#a-modules-self-signature)
  carries it in its keyworded channel.
- The old runtime's tutorial programs that use no feature beyond values,
  scopes and functions run on the rewritten stack and print the same output,
  and `tools/verify_snippets.py` reads the rewritten binary.
- The module's design doc is the `README.md` in its source directory, and the
  module's top-of-file comment links it.

**Directions.**

- *Builtins as function values — decided.* A builtin registers through the same
  bucket a user function does, as a function value whose body is native.
- *Newtype construction — decided.* An ordinary construction `(Head payload)`
  is `Tagged::construct`, the one construction rule the tie checks a knot's
  tagged nodes by too ([src/values/README.md](../../src/values/README.md#what-a-value-is)).
- *The lambda a combined operator or a quantified combined form binds —
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
  **candidate list**: the union, over the reader's scope and every enclosing
  one, of the registrations at that key whose position is below the reader's,
  filtered by the visibility predicate and sorted by the selection rule below,
  with the builtin's own overloads read off the builtin table. A keyworded use
  is an eager context, so a registration declared after it is invisible, as a
  function called before its `LET` is. The per-call residue is the type test
  alone: the first candidate every typed argument satisfies runs. An `EVAL`
  applies the same predicate at its own position when its block shape is built,
  collecting from every enclosing shape rather than stopping at the first that
  holds the key; a registration in evaluated code is a member of the `EVAL`'s
  block shape, a candidate for the statements after it there, and never widens
  a bucket around it, so no candidate list computed for a static site changes
  after it is built. The closed-bucket and overlap checks run where a shape is
  built, so evaluated code fails them at run time. [EVAL dynamic
  dispatch](eval-dynamic-dispatch.md) relaxes this later.
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
  a builtin over `Bool` and `!=` has no bucket.
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

**Requires:** none — the lattice, the elaborator and the quantified lambda ship.

**Unblocks:**

- [Modules](modules.md) — a module program runs only under dispatch.
- [EVAL dynamic dispatch](eval-dynamic-dispatch.md) — relaxes the shape-time candidate list for evaluated code.
- [Yielding iterators](yielding-iterators.md) — a demand for an element is an ordinary dispatch.
- [A compact type node table](compact-type-node-table.md) — its presize is calibrated on running programs.
