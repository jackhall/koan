# User-defined operator modules

A module-scoped surface for declaring operators — keyword, binary body,
precedence, and associativity — that populate the n-ary dispatch registry.

**Problem.** Operators are fixed in the compile-time
[`operators.rs`](../../src/parse/operators.rs) table (`.`/`?`/`!`), each wired to
a builtin builder. A user has no way to declare an operator — to bind a new
keyword to a binary body, place it in a precedence tier, or tag its
associativity — from their own module. The
[n-ary dispatch mechanism](n-ary-operators.md) recognizes and folds operator
chains and provides a per-scope operator registry, but nothing populates that
registry from user code: it has no declaration surface and no binder to write
into it.

**Impact.**

- *Users declare operators in their own modules*, scoped alongside the types the
  operator acts on.
- *A module bundles a precedence-ordered, associativity-tagged operator group*,
  so a whole family (`+ - * /`) registers and folds together.
- *Operator meaning is module-scoped and lexically resolved* — the same `+` can
  mean different things in different modules, picked by the scope walk.
- *Paired and grouped operators are user code.* A `+`/`-` pair or a `+ - * /`
  family is one module-declared operator group, not a set of hand-wired builtins.

**Directions.**

- *Declaration surface — open.* How a module spells its operator set, the binary
  body per operator, precedence ordering, and associativity. Single-operator form
  versus a paired/group form (e.g. a `GROUP + - OVER t` spelling that registers a
  family in one declaration); whether associativity is per-group or per-tier; how
  the body attaches. This is the gating decision; settle via `/design`.
- *Binder lowering — decided.* An `OP` skin over `FN` lowers a declaration to two
  registrations: the binary body into the ordinary function bucket under the
  operator keyword (so single binary calls dispatch with no special-casing), and
  the group record plus the size-≥2 powerset of member keys into the per-scope
  operator registry the [n-ary mechanism](n-ary-operators.md) walks. Mechanics
  decided; only the surface above is open.
- *Group validation — deferred.* Whether declared precedence/associativity (or
  the laws of an algebraic group declared over the operators) are checked at
  declaration or trusted is deferred to the surface decision and the
  property-testing engine.

## Dependencies

**Requires:**

- [User-definable n-ary operators](n-ary-operators.md) — the recognition,
  registry, and fold mechanism this surface populates.

**Unblocks:** none yet.

The shipped module system is a soft prerequisite — module-scoped operator
declaration needs modules to exist, but not the future modular-implicits stage.
Algebraic structures over these operators — group laws and generic-over-groups
functions — ride the modular-implicits stage on top of this surface; that payoff
lives with stage 5, not here.
