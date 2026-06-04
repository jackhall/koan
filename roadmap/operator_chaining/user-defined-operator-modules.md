# User-defined operator modules

A module-scoped surface — a `GROUP` skin over `OP` declarations — for declaring
operators, the chaining mode they reduce by, and (for pairwise) their combiner,
populating the n-ary dispatch registry.

**Problem.** Operators are fixed in the compile-time
[`operators.rs`](../../src/parse/operators.rs) table (`.`/`?`/`!`), each wired to
a builtin builder. A user has no way to declare an operator — to bind a new
keyword to a body, place it in a group, or choose how a run of it reduces — from
their own module. The [n-ary mechanism](n-ary-operators.md) reduces a recognized
run and provides a per-scope operator registry, but nothing populates that
registry from user code: it has no declaration surface and no binder to write
into it.

**Impact.**

- *Users declare operators in their own modules*, scoped alongside the types the
  operator acts on.
- *A `GROUP` bundles a mode-tagged operator family* (`+ - * /`), so a whole
  family registers and reduces together under one declaration.
- *Operator meaning is module-scoped and lexically resolved* — the same `+` can
  mean different things in different modules, picked by the scope walk.
- *Generic operator groups come from functors.* A `FUNCTOR` producing a `GROUP`
  module gives `+ - …` over any concrete type (or witness module), instantiated
  explicitly with no implicit-search machinery.

**Directions.**

- *`OP` form and `GROUP` surface — decided.* A binary operator is declared
  `OP <sym> OVER :Operand = (…)` — the `:` before the operand type is optional
  after `OVER` (`OVER Number` ≡ `OVER :Number`). It binds `left` and `right` to
  the two operands, and its result type defaults to `:Operand`, since a fold
  member shares operand and result types. A per-pair member of a pairwise group,
  whose result differs, adds an explicit return (`OP < OVER Number -> Bool = (…)`),
  still binding `left`/`right`; this heterogeneous form is valid **only inside a
  `PAIRWISE` group** — declared bare it errors, since without a combiner its
  self-run would left-fold ill-typedly (`(a < b) < c`). A unary operator is
  `UNARY OP <sym> OVER :Operand -> Result = (…)`, binding `operands` to the
  `:(LIST OF Operand)` run. `GROUP <Name> FOLD <LEFT|RIGHT> = ( OP … OP … )`
  skins `MODULE` and bundles fold members; `GROUP <Name> PAIRWISE FOLD <combiner>
  <LEFT|RIGHT> = ( … )` bundles per-pair members whose `<combiner>` is a function
  *value* named by its value-binding (`… PAIRWISE FOLD and LEFT …` references a
  `LET and = (FN …)`), folded over the per-pair results. A bare `OP` (no `GROUP`)
  still chains with *itself*: a bare binary op left-folds its run by default
  (`a + b + c` → `(a + b) + c`), a bare unary op's run collects to its `operands`
  body. A `GROUP` lets *distinct* operators mix in one run, and selects a
  non-default mode (fold-right, pairwise).
- *Combiner type-stability — decided.* A pairwise combiner is a fold over the
  per-pair result type `r` — a binary `(r, r) -> r` value — so a single
  application and a chained run share type `r`, and the per-pair body need not
  return `Bool`. Reductions that change type use the unary mode.
- *Binder lowering — decided.* An `OP` writes two things: its body into the
  ordinary function bucket under the operator keyword (a binary body for `a + b`,
  a list body for a unary op), and a single size-1 entry, keyed on its own
  keyword, into the per-scope operator registry the
  [n-ary mechanism](n-ary-operators.md) walks. That registry entry — not a
  function-bucket fallback — is what lets a bare `OP` self-chain: a bare binary
  op's entry records the fold-left default, a unary op's records unary. A `GROUP`
  additionally writes the size-≥2 powerset of its member keys, all resolving to
  one shared group record so distinct members mix; together with each member's
  size-1 entry (now carrying the group's declared mode), the full non-empty
  powerset resolves to the group.
- *Generics via functor — decided.* A generic group is a
  [`FUNCTOR`](../../design/typing/functors.md) producing the `GROUP` module,
  taking either a bare `:Type` parameter (when the bodies need no operations on
  the type) or a signature-typed module parameter (dictionary passing, when they
  do). Both are shipped functor mechanics. Implicit *selection* of a group by
  operand type rides modular implicits (stage 5), not this surface.
- *Group validation — deferred.* Whether the combiner's `(r, r) -> r` shape,
  mode consistency, or algebraic laws (associativity, identity, inverse) are
  checked at declaration or trusted is deferred to the property-testing engine
  and the group-laws stage.

## Dependencies

**Requires:**

- [User-definable n-ary operators](n-ary-operators.md) — the recognition,
  registry, and reduction mechanism this surface populates.

**Unblocks:** none yet.

The shipped module system and [functors](../../design/typing/functors.md) are
soft prerequisites — `GROUP` skins `MODULE`, and generic groups ride `FUNCTOR`.
Supplying a pairwise combiner needs a function *value*, today obtained with a
`LET and = (FN …)` before the group; a registration-free anonymous-value form
(anonymous functions and modules) would let it inline, but that is a separate
concern, not a prerequisite. Group laws and generic-over-groups *implicit*
dispatch ride the modular-implicits stage on top of this surface; that payoff
lives with stage 5, not here.
