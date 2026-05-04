# Expressions and parsing

This doc covers the parser pipeline, the `KExpression` shape it produces, the
language's eager-by-default evaluation rule (and how lazy slots opt out), and
how users extend the surface syntax through `FN` definitions rather than a macro
system.

## Parser pipeline

[`parse`](../src/parse/expression_tree.rs) runs in passes, one file each under
[src/parse/](../src/parse/):

1. [quotes.rs](../src/parse/quotes.rs) — replace string-literal contents with
   placeholders so later passes don't re-tokenize them.
2. [whitespace.rs](../src/parse/whitespace.rs) — turn indentation-based block
   structure into parenthesized form (2-space increments, no tabs).
3. [expression_tree.rs](../src/parse/expression_tree.rs) — walk the
   paren-delimited string into a nested expression tree.
4. [tokens.rs](../src/parse/tokens.rs) — classify each whitespace-delimited
   token as a literal, keyword (no lowercase — `LET`, `=`, `THEN`, `->`), type
   name (capitalized + has lowercase — `Number`, `KFunction`), identifier, or
   compound (member access, indexing, prefix/suffix operators). See
   [type-system.md](type-system.md) for what the three classes mean.
5. [operators.rs](../src/parse/operators.rs) — table of compound-token
   operators (`!`, `.`, `[]`, `?`); add a row to extend.

## `KExpression` shape

Output is one [`KExpression`](../src/parse/kexpression.rs) per top-level line:
an ordered sequence of `ExpressionPart`s — `Keyword`, `Identifier`, `Type`,
nested `Expression`, `ListLiteral`, `DictLiteral`, or typed `Literal`.

The `Keyword`-vs-slot split is the parser's contract with dispatch:

- `Keyword` parts contribute fixed tokens to a signature's bucket key (the part
  that has to match exactly).
- `Identifier`, `Type`, literals, and sub-expressions become slots that compete
  on type specificity (see [type-system.md](type-system.md)).

`KExpression` is itself a first-class `KObject` variant — user code can hold an
unevaluated expression as a value, pass it around, and (eventually, with quote
sigils) evaluate it on demand.

## Eager evaluation by default

The scheduler evaluates every nested `(...)` before its parent dispatches. So:

```
IF p THEN x
```

is a **post-hoc selector**, not a short-circuit — `x` is evaluated whether or
not `p` is true, and then `IF/THEN` chooses what to return. This is a
deliberate consequence of the graph-based execution model: the parent slot's
arguments are dependencies in the DAG, and the topological order of execute
makes them ready before the parent runs. See
[execution-model.md](execution-model.md).

## Lazy slots

A builtin can opt out of eager evaluation for specific slot positions: it
declares the slot as lazy at registration, the scheduler hands it the
unevaluated `KExpression` instead of a value, and the builtin emits a fresh
`Dispatch` for the chosen branch only. Historically `if_then`'s lazy slot used
[`SchedulerHandle::add_dispatch`](../src/dispatch/scope.rs); today
[`BodyResult::Tail`](../src/dispatch/kfunction.rs) is the standard mechanism (a
deferring builtin tail-returns the chosen branch and the scheduler dispatches it
in place).

## Extending the surface

Users add what look like new keyword forms by writing `FN` definitions.

```
FN (LOOP body) -> Any = (...)
```

defines a new dispatchable signature: keyword `LOOP`, slot `body`. The parser
already classifies `LOOP` as a keyword (all-caps, no lowercase), and
`body` as a slot. So the call site `LOOP (PRINT "x")` is parsed and dispatched
the same way a builtin would be — the dispatch table doesn't distinguish
user-defined from built-in functions when scoring matches.

There is no macro system. The dispatch table **is** the language's extension
mechanism. Two consequences:

- New "syntax" cannot rewrite the parser. It can only introduce new dispatchable
  shapes within the existing token grammar.
- A user-defined function competes with builtins on slot-specificity, so a
  more-specific user signature can override a more-general builtin where the
  shapes overlap.

See [functional-programming.md](functional-programming.md) for how the body
substitutes parameters and what `BodyResult::Tail` does at the slot.

## Open work

- [Quote and eval sigils](../roadmap/quote-and-eval-sigils.md) — no surface
  form to force-evaluate a metaexpression or suppress evaluation inside a
  dict/list literal. Closes the gap between "`KExpression` is a first-class
  value" and "user code can manipulate expressions ergonomically".
- Source spans on `KExpression`
  ([deferred-surface-items.md](../roadmap/deferred-surface-items.md)) — error
  frames currently can't point to a line/column in source.
