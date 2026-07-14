# Operators

[`OP`](../src/builtins/op_def.rs) declares a chainable operator in the enclosing
scope; [`GROUP`](../src/builtins/group_def.rs) bundles mutually chainable
operators under one reduction mode. Together they are the declaration surface that
populates the per-scope operator registry the
[chain reducer](expressions-and-parsing.md) walks — the reducer decides *how* a
recognized run reduces, and this surface decides *what* is a run and what each
operator does.

Both are ordinary builtins: they add no dispatch-classifier case and no reserved
lead keyword. What buys that is the quote.

## The symbol is quoted

An operator symbol arrives as a `#(...)` quote — a parse-static
[`QuotedExpression`](../src/machine/model/ast.rs) part, captured by the parser as
data. A quote is a *slot* for dispatch purposes, so every `OP` / `GROUP` overload
keeps a **fixed** untyped key and matches whatever symbol it is handed:

```
OP #(+) OVER Number = (left + right)
```

An unquoted `+` would be a `Keyword` part, which lands in the expression's own
untyped key — so every operator would key a different bucket and no fixed overload
could match. The quote is what makes the symbol an argument rather than syntax.
The same rule governs a pairwise group's combiner (`PAIRWISE FOLD #(BOTH) LEFT`).

The declaration surface's own spelling is reserved: `OP`, `UNARY`, `OVER`,
`GROUP`, `FOLD`, `PAIRWISE`, `LEFT`, `RIGHT`, `=`, `->`, `:|`, `:!` cannot name an
operator. Every other keyword-classified token can, including an all-caps
alphabetic name (`OP #(MAX) OVER Number` is fine).

## Binary operators

```
OP #(<sym>) OVER <Operand> = (<body>)
OP #(<sym>) OVER :<Operand> = (<body>)          -- the `:` is optional after OVER
OP #(<sym>) OVER :(LIST OF Elt) = (<body>)      -- or a sigiled type expression
```

The body binds `left` and `right`, both of the operand type, and its result type
*is* the operand type — a fold member feeds its result back in as the next
operand, so the two coincide. The body captures its declaring scope, so it sees
its sibling module bindings exactly as a bare `FN` body does, and the declaration
evaluates to the function it declares.

A member of a **pairwise** group has a result type of its own, and says so:

```
OP #(<) OVER Number -> Bool = (…)
```

Such a heterogeneous form is admissible *only* inside a `PAIRWISE` group, where a
combiner folds the pair results. Declared anywhere else it errors: without a
combiner its own run would left-fold ill-typedly (`(a < b) < c`).

## Unary operators

```
UNARY OP #(<sym>) OVER <Operand> -> <Result> = (<body>)
```

A unary operator takes the **whole run as one list**: the body binds `operands`
of type `:(LIST OF Operand)`, and infix (`a ~ b ~ c`) and prefix (`~ [a b c]`)
forms reduce to the same keyword-first call. The `-> Result` segment is mandatory
— the body consumes a list of operands, so there is nothing to default the result
type from. A two-operand use (`a ~ b`) names one keyword and so dispatches as a
plain keyworded call rather than a chain; the declaration registers a synthesized
binary **bridge** whose body is `sym [left right]`, so that surface lands on the
one list body the user wrote.

Because it chains with nothing, a unary operator can be no group's member.

## Groups

A `GROUP` **is** a module: it binds a module value under a snake_case name, its
body is an ordinary module body, and `USING <group> SCOPE (…)` opens it. What it
adds is one shared registry record, so *distinct* members mix in a single run.

```
GROUP vec_ops FOLD LEFT = (
  (OP #(+) OVER :(LIST OF Number) = (…))
  (OP #(-) OVER :(LIST OF Number) = (…)))

GROUP num_compare PAIRWISE FOLD #(BOTH) LEFT = (
  (OP #(BOTH) OVER Bool = (…))          -- the combiner, over the pair-result type
  (OP #(≺) OVER Number -> Bool = (…))
  (OP #(≼) OVER Number -> Bool = (…)))
```

`FOLD LEFT` / `FOLD RIGHT` give the group `FoldLeft` / `FoldRight`;
`PAIRWISE FOLD #(<combiner>) <LEFT|RIGHT>` gives it `Pairwise`, carrying the
combiner symbol and the direction the pair results fold in.

The members are read off the **unevaluated** body — a structural scan of its
top-level `OP` statements — and the full nonempty powerset of the member set is
registered into the group's child scope *before a single body statement runs*, all
subsets pointing at the one record. So declaration order inside the body does not
matter, and a mixed-member run reduces inside the group's own body as readily as
through a `USING` window. Only top-level `OP` statements are members: an `OP`
nested inside an `FN` or a branch declares an operator in *that* scope and joins
no group. Any other statement — a `LET`, an `FN`, the combiner's own `OP` — is
ordinary module content.

A group is the sole registrar for its members: a member `OP` writes the function
bucket only, while a *bare* `OP` (outside any group) also writes a size-1 registry
entry, so it self-chains — fold-left for a binary operator, unary for a
`UNARY OP`.

A Type-token group name takes the same respelling diagnostic `MODULE`'s
Type-named overload reports: a group is a module, and a module is a value.

## The pairwise combiner is an operator, invoked infix

A pairwise run dispatches each adjacent pair through its own member's body and
folds the pair results through the group's combiner. The record stores the
combiner's **symbol**, never a resolved function, and the reducer synthesizes the
infix shape `[left, Keyword(<combiner>), right]` at each use site. That shape
re-enters ordinary keyworded dispatch, so the combiner binds its two inputs
**positionally**, by signature shape — it imposes no parameter-naming convention
on the combiner, and it reaches a type-level combiner, which the function-value
lane could not.

In practice a combiner is therefore declared as an `OP` over the pair-result type
`r`, which makes the `(r, r) -> r` fold shape true by construction. Declaring it
inside the group body carries it through `USING` alongside the operator bodies.

Resolution is the ordinary scope walk at the chain's use site, so a combiner that
is missing, non-callable, or of the wrong arity surfaces as an ordinary error
*there*, not at declaration. Storing a name rather than a function is also what
keeps [`OperatorGroup`](../src/machine/model/operators.rs) lifetime-free: the
record borrows no region, so its allocation door stays a trivial no-op gate.

## Shadowing is type-gated, not forbidden

An operator declaration may name a symbol the builtins already use. The two
lookups it touches resolve differently, and between them that is exactly the
behavior wanted:

- The **function bucket** is builtin-first: dispatch consults the immutable
  run-global root before the scope walk, so the builtin `+` still wins for the
  operand types it declares (`Number`), and only other operand types fall through
  to a module's own body. `OP #(+) OVER :(LIST OF Number)` therefore adds list
  addition and leaves arithmetic alone.
- The **operator registry** is innermost-wins
  ([`Scope::resolve_operator_group_with_chain`](../src/machine/core/scope.rs)): a
  registry hit carries a member set and a mode but no operand types, so it *cannot*
  type-gate. The builtin groups seeded into the root are found last — they are
  chaining defaults a declaring scope may override, which is what lets a scope
  declare `-` as `FOLD RIGHT` and have its own runs right-associate.

Within one scope, one operator has one chaining mode: two `OP` statements over the
same symbol and distinct operand types are two bucket overloads and one registry
entry (an idempotent upsert), while two that disagree on the mode are an error.

## Visibility

An `OP` writes into its **enclosing scope** — a module body's child scope, a
`GROUP`'s child scope, a per-call `FN` scope, or the top level — and a use site
finds it by the ordinary innermost-wins scope walk with lexical cutoff. The
`USING` window's borrowed façade surfaces a module's operator registrations
alongside its values and function overloads, so opening a group puts both its
member bodies and its chaining mode in scope
([modules.md § Block-scoped opening](typing/modules.md#block-scoped-opening-using--scope)).

An operator declared *after* a run is invisible to it (lexical cutoff), while one
declared before it in the same submitted block resolves whatever order the
scheduler pops the statements in: the `OP` binder installs a pending-overload entry
under each bucket key its body will register, and a chain that misses the registry
parks on a visible pending declaration rather than erroring.

## Generic groups are functors

A group parameterized by a type is an ordinary
[functor](typing/functors.md) — an `FN` whose body is a `GROUP`:

```
LET make_ops = (FN (MAKEOPS Elt :Type) -> Module = (
  GROUP result FOLD LEFT = (
    (OP #(+) OVER :(LIST OF Elt) = (…))
    (OP #(-) OVER :(LIST OF Elt) = (…)))))
```

Instantiate it at a concrete type (`MAKEOPS Number`) when the member bodies need
no operation on the element type, or at a witness module satisfying a signature
(dictionary passing) when they do. Both are shipped `FN` mechanics; the group adds
nothing.

Selection is always **explicit**: an instantiation binds a module, and only a
`USING` window over that module surfaces its operators. No group is chosen for a
run by the run's operand type.

## Open work

- [Stage 4 — Property testing and axioms](../roadmap/predicate_typing/axioms-and-generators.md)
  — group validation: whether a combiner's `(r, r) -> r` shape, mode consistency,
  and algebraic laws (associativity, identity, inverse) are checked at declaration
  or trusted.
- [Stage 5 — Modular implicits](../roadmap/predicate_typing/modular-implicits.md)
  — implicit *selection* of a group by operand type, so a run finds its group
  without a `USING` window.
- [Unify the unary-operator registration shape](../roadmap/operator_chaining/unify-unary-operator-registration.md)
  — the builtin `|` hand-rolls the same registration triple `UNARY OP` synthesizes.
- [Reject the unquoted operator symbol](../roadmap/operator_chaining/reject-the-unquoted-operator-symbol.md)
  — an unquoted `OP (+)` still registers, but installs no park edges and errors
  inside a `GROUP` body; the quote is the sole spelling this doc gives.
