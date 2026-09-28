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
program runs on the rewritten stack. The shape builder records a binder only at
a statement's root, so a binder nested in an expression,
`PRINT (LET doubled = 42)`, declares nothing and is not refused.

**Acceptance criteria.**

- Dispatch keys a keyworded registration on its full bucket key, and a typed
  argument that does not satisfy a candidate is a non-match that falls through
  rather than a bind-time error.
- A call runs the admitting candidate the priority-class order ranks first; two
  admitting candidates it leaves unordered, with no builtin among them to win
  the tie, are an ambiguity error, whether one scope declares both or two do,
  and no admitting candidate is a no-overload error naming the arguments' types.
- `SHOW 1 WITH "s"` selects the definition headed
  `#(SHOW x :Number WITH y :Any)` over the one headed
  `FOR ALL #[Elt] #(SHOW x :Elt WITH y :Str)`: the first class decides.
- `MOVE "s" TO 1` selects the definition headed
  `#(MOVE x :(Str | Bool) TO y :Number)` over the one headed
  `#(MOVE x :(Number | Str) TO y :Any)`: a class that orders neither passes
  both to the next.
- A definition headed
  `FOR ALL #[Elt] #(PAIR x :(LIST OF Elt) WITH y :(LIST OF Elt))`, called by
  keyword, refuses `PAIR [1] WITH [1, "x"]` and admits
  `PAIR [1, "x"] WITH [1]`; under the declaration `EXPR #(PAIR 1 WITH 1)`, or
  called by name as a `FN EXPR`, it admits both.
- `PAIR 1 WITH 2` selects the definition headed
  `FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt)` over the one headed
  `FOR ALL #[First Second] #(PAIR x :First WITH y :Second)`.
- `TAKE [1] WITH 1` selects the definition headed
  `#(TAKE x :(Str | (LIST OF Number)) WITH y :Number)` over the one headed
  `FOR ALL #[Elt] #(TAKE x :(Number | (LIST OF Elt)) WITH y :Elt)`: where the
  first class orders neither, `Elt` reads as its bound.
- A bucket declaration `EXPR #(MOVE 2 TO 1)` ranks `MOVE`'s second slot in the
  first class for every definition written where it is visible, and a
  definition's head carries no integers.
- Two declarations of one ranking at one key are one declaration, so two
  libraries that each declare `EXPR #(MOVE 2 TO 1)` compose. Rankings that
  disagree are refused where a shape is built, wherever two meet: a declaration
  or definition that sees another, a candidate list, a `USING` fill, or a
  signature meet or ascription. A definition with no declaration visible
  carries the written-order ranking, which disagrees with `2 … 1`.
- A ranking is part of the expression shape type:
  `:(EXPR #(MOVE 2 :Any TO 1 :Any) -> Any)` and
  `:(EXPR #(MOVE 20 :Any TO 10 :Any) -> Any)` are one type, and
  `:(EXPR #(MOVE _ :Any TO _ :Any) -> Any)` is another, so a module whose
  `MOVE` carries the written-order ranking does not satisfy a signature member
  ranked `2 … 1`.
- A keyworded use with no candidate at all — no builtin overload and no visible
  registration at its key, whatever declarations it sees — is refused where its
  shape is built, as an unbound name is.
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
  binder first. A candidate list keys on the full bucket key: a registration
  at `MOVE _ TO _` is no candidate for `MOVE _`.
- A binder nested among an expression's eager parts, as in
  `PRINT (LET doubled = 42)`, is refused where the shape is built.
- A combined expression shape — `LET f = FN EXPR …`, `LET plus = OP …` — binds
  a lambda to its name and registers its expression shape under its bucket:
  [`callable_type`](../../src/elaborate/signature.rs) already hands a named
  callable [its function type](../../src/elaborate/README.md#a-callables-type),
  quantified where the expression shape carries a `FOR ALL` group, so only a
  bucket registration carries an `ExpressionShape`.
- A `FN` no binder names — a body's last statement, the head of a call, an
  argument, a data member's part the tie asks for — is born where dispatch
  evaluates it, through the [lambda door](../../src/knot/README.md#a-lambda):
  tutorial 04's `CONSTANTLY` returns one, which is called afterward with the
  captures it was born with.
- A bucket-only definition — a bare `EXPR` or `OP` statement — is a bound member
  of the activation it is declared in, and a module's
  [self-signature](../../src/elaborate/README.md#a-modules-self-signature)
  carries it in its keyworded channel; a combined expression shape's bucket is
  carried there beside its named value slot.
- A bare `EXPR FOR ALL` called by keyword binds each type parameter to its
  group's solution against the arguments, which a test reads in the body.
- A call in a body's tail position whose declared return satisfies the
  caller's runs as a tail hop: a tail recursion N deep holds O(1) cells.
- A frame's value satisfies its callee's declared return and carries it — a
  container retyped to the declared type, a tagged value to the union member
  naming its constructor — or the call yields an error.
- A registration whose bucket key holds no keyword, anywhere in it, is
  refused where its shape is built.
- `ATTR p y`, and `LET which = #(y)` followed by `ATTR p (which)`, read the
  same field, and `ATTR` over a `Str` label is a no-overload miss.
- `EVAL n` over a number is a no-overload miss, not a check of `EVAL`'s own, and
  the `EVAL` door's `CodeRefused::NotCode` is deleted.
- A keyworded use written unmarked in a quote has as candidates the builtin
  table's overloads, the registrations composed ahead of it, and those a `USING`
  of a module fills it with, so a user overload of `PRINT` or `==` composed
  ahead of the use is selected in the builtin's place, while one visible only
  where the quote is written or run is not a candidate.
- `#(GREET "bob") USING m` fills the keyworded hole with `m`'s registrations at
  `GREET`'s key, as it fills a name hole with `m`'s member.
- `$(…)` resolves the bucket key of the use it wraps where the quote is written,
  and `\(…)` where its code is built; over an operator run, each marks every use
  the chain builds, so `$(a < b < c)` resolves both `<` uses and the `AND`
  joining them where the quote is written.
- A `NEEDING` list names bucket keys beside names: a parameter admits a quote
  whose needed keys its list covers, and an `EVAL` of the parameter resolves each
  listed key where the `EVAL` is written.
- A builtin function equals only itself, by its table identity.
- `EVAL` and `code USING src` run as koan expressions through the doors
  [the program](../../src/program/README.md#the-body-runner) supplies, so `TWICE #(PRINT $x)` prints the
  caller's `x`.
- An evaluation or a frame that cannot proceed yields a koan error value, every
  evaluation passes an error it receives through unchanged, and an uncaught one
  ends the program with `error: <message>`.
- Every runnable tutorial snippet that uses no `MATCH`, `TRY`, `CATCH`,
  `Result`, `MODULE`, `SIG`, `VAL`, `TYPE`, `USING … SCOPE`, `:|`, `:!` or `CLOSE` runs
  on the rewritten stack and prints the output its tutorial shows, and
  `tools/verify_snippets.py` reads the rewritten binary, skipping
  the snippets that use an expression shape on its pending list.
- The module's design doc is the `README.md` in its source directory, and the
  module's top-of-file comment links it.

**Directions.**

- *What dispatch runs — decided.* Literals, names, containers, record access
  (`.`, `ATTR`, `FROM`), newtype, type-constructor family and union-variant
  construction, functions, keyword dispatch, the builtin library, quotes,
  `EVAL` and `USING` over code, and uncaught errors. [Control expression shapes and
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
  `FN :{operands :(LIST OF Number)} -> Number`. A `FN EXPR FOR ALL #[Elem] …`
  binds the quantified function type `callable_type` builds for it.
  Quantification changes nothing about how a call by name binds its frame:
  arguments bind by name, the frame
  [solves the group](../../src/program/README.md#the-body-runner) against them
  jointly, and a typed argument the callee's group cannot be solved against is the
  ordinary type mismatch.
- *Keyword reads resolved in the shape — decided.* A bucket key resolves when
  the shape is built, as a value or type name does
  ([src/scope/README.md](../../src/scope/README.md#resolution)), to a
  **candidate list**: a fixed run of coordinates — the builtin's own overloads
  read off the builtin table, then the registrations at that key visible to the
  use in each enclosing scope, filtered by the visibility predicate. Inside a
  `USING … SCOPE` body the registrations its operand surfaces are candidates
  too, held as the body's parameters as its surfaced names are. A keyworded use is a **mention of each candidate**, classified
  eager or deferred as a name mention is: at a statement it reads at the
  statement's position, so a registration declared after it is invisible, as a
  function called before its `LET` is; inside a callable body it is deferred, so
  the body sees its own registration and later ones, captures them, and
  mutually recursive registrations tie as one knot. A candidate's slot types are
  elaborated when its function is born, so no order is fixed where the shape is
  built; the per-call residue is the type test and the selection rule below. A
  keyworded use inside a quote is a hole unless marked: its candidates are the
  builtin table's overloads, the registrations composed ahead of it, and those
  a `code USING src` fills it with from a module's keyworded channel, while
  `$(…)` resolves its bucket key where the quote is written and `\(…)` where
  its code is built ([holes and marks](../../src/scope/README.md#holes-and-marks)); a registration in
  evaluated code is a member of the block shape it runs in, a candidate for the
  statements after it there, and never widens a bucket around it, so no
  candidate list computed for a static site changes after it is built. The
  closed-bucket and overlap checks run where a shape is built, so code composed
  at run time fails them when it is evaluated.
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
  admits the arguments' carried types and ranks them lexicographically by
  priority class, through the type lattice's shape order
  ([`shape_specificity`](../../src/type_lattice/sig_relations.rs)), the order a
  signature's bucket replay ranks by too, so dispatch compares no slot types of
  its own. At each class, a candidate is at least as specific as another when
  the other's slots in that class admit its own, jointly, with each variable an
  earlier class solved read as that solution; where that earlier class did not
  admit, the variable reads as its bound. Every survivor no other survivor
  strictly beats at a class goes on to the next, so a class that orders neither
  of two candidates leaves them to later classes. A lone survivor runs; where
  several remain, a builtin among them wins, and otherwise the call is an
  ambiguity error, wherever they were declared: no scope shadows another's
  overload. The overlap check runs at load over the operand types spelled from
  builtin names alone, so an operand type known only at birth is never checked.
- *Priority classes — decided.* A bucket declaration writes an integer in a
  slot's place, `EXPR #(MOVE 2 TO 1)`, and the slot sits in that priority
  class; a `_` slot is a class of its own, after the numbered ones, in written
  order, and slots sharing an integer are one class. A definition's head
  carries no integers, so no slot is a triple of rank, name and type: a
  definition takes its ranking from the declaration visible where it is
  written, and with none it ranks its slots in written order. The ranking is
  part of the expression shape type, normalized to dense ranks and fed to its
  digest and rendering, so the shape order and
  [`admits_shape`](../../src/type_lattice/sig_relations.rs) read it off the
  type. An operator's slots carry no integers: fold-left ranks `left` first,
  fold-right `right` first, pairwise runs left to right, and a unary operator
  has one slot.
- *Admission in priority order — decided.* A keyworded call solves a quantified
  candidate's group class by class: the first class holding a variable solves
  it, jointly over that class's slots and every position inside them, and each
  later class admits its arguments against that solution. A record's fields
  keep no order, so a slot's own positions always solve jointly, and so does a
  call by name: naming the callee picks one implementation, so it may admit
  what a keyworded call of the same function refuses. The shape order's
  instantiation clause solves by the same classes through
  [`admits_shape`](../../src/type_lattice/sig_relations.rs), so a signature
  view never promises a call its satisfying overload refuses.
- *Ranking fixed per candidate list — decided.* The order reads the
  candidates' shape types only — declared slot types and their shared
  ranking — so whether one candidate is at least as specific as another at a
  class is a function of their two shape types and the class. It is a relation
  of the type lattice, recorded in the registry's verdict table the first time
  a call needs it, so a later call runs the class-by-class elimination over its
  admitting candidates by reading verdicts rather than comparing slot types.
- *A selection cache per call site — decided.* Not built here. Keying the
  selection on the candidate list's births and the tuple of the arguments'
  carried types would let a monomorphic site skip admission and ranking, but
  only once selection is shown to depend on nothing else; a site in a callable
  body would key on the births its activation resolved, since its candidates
  are born per activation. The idea moves to the rewrite's
  [unplanned work](README.md#unplanned-work) when this item ships.
- *One order per keyword pattern — decided.* Every declaration and definition
  at a bucket key carries one ranking, checked where a shape is built, so no
  call is refused for its ordering. Letting candidates disagree at a later
  class so long as each step's survivors agree would make a call's legality
  depend on its arguments' types, since survival reads slot types born later;
  CLOS's `:argument-precedence-order`, a property of the generic function that
  every method shares, is the precedent. A declaration is idempotent: two of
  one ranking are equal content, so libraries that declare a keyword pattern
  alike compose, and one that disagrees is refused wherever the two meet.
  Nothing indexes shapes by bucket key, so there is no run-wide bucket state,
  and two libraries that never meet never conflict. A definition with no
  declaration carries its written-order ranking as an implicit one, so a
  library's overloads never re-rank when another's declaration is present, and
  an inner declaration that disagrees with an outer one is refused rather than
  shadowing it, since a candidate list sees both scopes.
- *A bucket declaration — decided.* A bodiless `EXPR` statement of keywords and
  one integer or `_` per slot, `EXPR #(MOVE 2 TO 1)`, declares its bucket's
  ranking and nothing else: it spells no types and no return, and in a module
  body it declares no member. A declaration that bounded its definitions'
  types would make two libraries sharing a keyword pattern agree on types as
  well as order. It is not a candidate, so a use that sees a declaration and no
  definition is refused where its shape is built. A `SIG` member writes an
  integer in `_`'s place, `EXPR #(MOVE 2 :Piece TO 1 :Square) -> Board`, and a
  `MODULE` or `GROUP` annotated with a signature takes that signature's ranked
  member heads as declarations in its body ([modules](modules.md)).
- *Errors — decided.* A koan error is a tagged value of the builtin nominal
  `Error` over `{message :Str}`, and a consumer checks the results it reads, per
  the [scheduler](../../src/scheduler/README.md#the-drain). The program stops at
  the first uncaught error. [Control expression shapes and
  errors](control-and-errors.md) widens the payload when `CATCH` needs more.
- *Tail calls — decided.* A frame evaluates its last statement in its own cell,
  with its declared return as the contract; when that statement's selected call
  returns a type satisfying the contract, the cell hops to the callee's frame
  instead of spawning it, and otherwise it checks the value on finish.
- *The builtin library — decided.* `PRINT`; `+ - * /` over `Number`;
  `< <= > >=` over `Number`, returning `Bool`; `AND` and `NOT` over `Bool`;
  `==` over `Any`; `|` and `&` over types, as the infix pair and the unary list
  form; and the `BUILTIN_SHAPES` entries whose slots are evaluated — `ATTR`,
  `FROM`, `EVAL` and `USING`. There is no builtin `OR` and no unary minus.
  `ATTR` over a module waits on [modules](modules.md).
- *The interpreter binary — decided.* `src/main.rs` becomes the rewrite's
  interpreter, built by default, and `tools/verify_snippets.py` reads it.
- *What the tutorial shows — decided.* A snippet whose output the rewrite
  changes — an error's text, or a behaviour the rewrite's design changed — is
  rewritten to the rewrite's output. `tools/verify_snippets.py` skips a snippet
  that uses an expression shape on a pending list, which later items shrink.
- *An overload that is never selected — decided.* A functor is a `FN` or
  `EXPR` returning a module, so an `OP #(+) OVER Elt` in its body learns its
  operand type per call: at `Elt = Number` the builtin is selected first,
  inside the functor's own body too. Two user overloads that meet at the
  instantiated type are an ambiguity error at the call instead, so only a
  builtin leaves a user overload unselected. Nothing reports it:
  koan has no warning channel, and the case is recorded as the motivation for
  one under [unplanned work](README.md#unplanned-work).
- *Visibility shared with operator groups — decided.* An operator run sees a
  user's [operator group](../../src/scope/README.md#operator-groups) inside the
  `GROUP`'s own body and
  inside a `USING … SCOPE` body surfacing it, which is where the group's
  overloads are admitted here; builtin chaining is seen everywhere.
- *A bucket key in a `NEEDING` list — decided.* It is stored as a symbol, the
  key itself, and is written and rendered with `_` for each slot,
  `:(Block NEEDING #[(LOG _)])`: a slot name is invisible to dispatch, so the
  list never names one.
- *A group mark over an operator run — decided.* `$(a + b + c)` wraps one
  written node that chains into two uses, and the mark covers every use the
  chain builds, a pairwise operator run's combiner included, so every operator
  in the operator run resolves the same way. A mark over `a != b` covers the
  `==` the rewrite builds; the `NOT` it wraps that in is always the builtin's.
- *A shape built before composition — decided.* A hole's candidate list at an
  open bucket includes registrations composed ahead of it, so a code shape
  built before composition is never reused after it: composed code builds its
  own ([code splicing](code-splicing.md)). A quote argument is matched by its
  value's carried type, which names what its code needs, never by the raw
  part's code kind.
- *What dispatch evaluates — decided.* The statements a body's shape owns,
  [rewritten](../../src/scope/README.md#operator-groups), never the parse; and an
  expression part holding a nested block shape runs as a block whose value is
  its last statement's, which is how a pairwise operator run's shared operand
  evaluates once.

## Dependencies

**Requires:**


**Unblocks:**

- [Modules](modules.md) — a module program runs only under dispatch.
- [Yielding iterators](yielding-iterators.md) — a demand for an element is an ordinary dispatch.
- [A compact type node table](compact-type-node-table.md) — its presize is calibrated on running programs.
- [Control expression shapes and errors](control-and-errors.md) — arms run as blocks, and errors are values, here.
- [Slicing and splicing](slicing-and-splicing.md) — slicing and splicing are builtins.
- [Dict defaults](dict-defaults.md) — a dict lookup, which is what observes a default.
