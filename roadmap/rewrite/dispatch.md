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
- A reference to a visible binder that has not yet bound parks until it binds,
  and a dispatch placeholder keys on the full bucket key.
- A combined form — `LET f = FN EXPR …`, `LET plus = OP …` — binds a lambda to
  its name and registers its expression shape under its bucket:
  [`callable_type`](../../src/elaborate/signature.rs) hands a named callable a
  function type, and only a bucket registration carries an `ExpressionShape`.
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
- *The lambda a combined operator or a quantified combined form binds — open.*
  A binary `OP` body's parameters are `left` and `right` and a unary one's is
  `operands`, so `FN :{left :Number, right :Number} -> Number` is the candidate
  for `LET plus = OP #(⊕) OVER Number`; a `FN EXPR FOR ALL (Elem) …` needs a
  function type to carry a quantifier group, which `KFunction` does not.
- *Keyword reads resolved in the shape — open.* A `BUILTIN_SHAPES` entry's
  bucket is closed, so a node matching one resolves when the shape is built;
  every other bucket is overloaded, and a user bucket shadowable. Recommended:
  extend the shape's coordinate resolution to bucket keys, as value and type
  names resolve
  ([src/scope/README.md](../../src/scope/README.md#resolution)).
- *Builtin buckets a user adds to — decided.* A `BUILTIN_SHAPES` entry is closed
  at its key because the body-shape builder walks a matching node by the entry's
  roles. An operator's bucket is open by type because its slots are eager
  operands whichever overload is selected, so no walk depends on the selection.
  How an operator run of it chains is [operator groups](operator-groups.md)' and
  is never a user's to change.
- *Shadowable builtins — open.* `==` takes `Any`, so under builtin-first
  selection no user overload of it is ever selected; the kind exists so a
  user-defined equality is possible. A user's `==` returns `Bool` and `!=` is
  never declared: [operator groups](operator-groups.md) holds a program to both
  and rewrites `a != b` as `NOT (a == b)`, so `NOT` is a builtin over `Bool`
  and `!=` has no bucket. Which other builtins
  belong to the kind, and its selection rule, are undecided — "most specific
  wins, a builtin wins a tie" may serve this kind and the operator kind alike.
- *An overload that is never selected — open.* A functor is a `FN` or `EXPR`
  returning a module, so an `OP #(+) OVER Elt` in its body learns its operand
  type per call: at `Elt = Number` the builtin is selected first, inside the
  functor's own body too, and nothing reports it. Two user overloads meet the
  same way when an enclosing scope already holds the instantiated one. Koan has
  no warning channel; what reports the overload where it is born is undecided.
- *Visibility shared with operator groups — decided.* An operator run sees a
  user's [operator group](operator-groups.md) inside the `GROUP`'s own body and
  inside a `USING … SCOPE` body surfacing it, which is where the group's
  overloads are admitted here; builtin chaining is seen everywhere.
- *What dispatch evaluates — decided.* The statements a body's shape owns, which
  [operator groups](operator-groups.md) rewrites, never the parse; and an
  expression part holding a nested block shape runs as a block whose value is
  its last statement's, which is how a pairwise operator run's shared operand
  evaluates once.

## Dependencies

**Requires:**

- [The top level on the scheduler](top-level-on-the-scheduler.md) — running a program needs the scheduler that drives it.

**Unblocks:**

- [Modules](modules.md) — a module program runs only under dispatch.
- [Yielding iterators](yielding-iterators.md) — a demand for an element is an ordinary dispatch.
