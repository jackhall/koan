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
   token as a literal, keyword (pure-symbol like `=`, `->`, `:|`, `:!`, or
   alphabetic with ≥2 uppercase letters and no lowercase — `LET`, `THEN`),
   type name (uppercase-leading with at least one lowercase — `Number`,
   `KFunction`, `IntOrd`), identifier, or compound (member access, indexing,
   prefix/suffix operators). A token that starts uppercase but classifies as
   neither keyword nor type (single uppercase letter, or uppercase + digits
   only) is a parse error. See
   [typing/tokens.md](typing/tokens.md)
   for what the three classes mean.
5. [operators.rs](../src/parse/operators.rs) — table of compound-token
   operators (`!`, `.`, `[]`, `?`); add a row to extend.

## `KExpression` shape

Output is one [`KExpression`](../src/machine/model/ast.rs) per top-level line:
an ordered sequence of `ExpressionPart`s — `Keyword`, `Identifier`, `Type`,
nested `Expression`, `ListLiteral`, `DictLiteral`, or typed `Literal`.

The `Keyword`-vs-slot split is the parser's contract with dispatch:

- `Keyword` parts contribute fixed tokens to a signature's bucket key (the part
  that has to match exactly).
- `Identifier`, `Type`, literals, and sub-expressions become slots that compete
  on type specificity (see [typing/ktype.md](typing/ktype.md)).

`KExpression` is itself a first-class `KObject` variant — user code can hold an
unevaluated expression as a value, pass it around, and evaluate it on demand.

## Eager evaluation by default

The scheduler evaluates every nested `(...)` before its parent dispatches. So
without further machinery,

```
MATCH cond WITH (true -> (a) false -> (b))
```

would evaluate both `(a)` and `(b)` regardless of `cond`, and `MATCH` would
just be a post-hoc selector picking one of the two already-computed values.
This is a deliberate consequence of the graph-based execution model: the
parent slot's arguments are dependencies in the DAG, and the topological order
of execute makes them ready before the parent runs. See
[execution-model.md](execution-model.md). To get real branching behavior,
`MATCH` opts its branch slots into laziness — the next section.

## Lazy slots

A builtin can opt out of eager evaluation for specific slot positions: it
declares the slot as lazy at registration, the scheduler hands it the
unevaluated `KExpression` instead of a value, and the builtin emits a fresh
`Dispatch` for the chosen branch only. Two mechanisms exist:
[`SchedulerHandle::add_dispatch`](../src/machine/core/scope.rs) submits a child
node directly, while [`BodyResult::Tail`](../src/machine/core/kfunction.rs) — used
by `MATCH` — tail-returns the chosen branch so the scheduler dispatches it in
place.

## Extending the surface

Users add what look like new keyword forms by writing `FN` definitions.

```
FN (LOOP body :KExpression) -> Any = (...)
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

## Quote and eval sigils

Two prefix sigils give surface to the lazy/eager split: `#(expr)` *quotes* —
captures the body's AST as a `KObject::KExpression` value with no evaluation —
and `$(expr)` *evals* — resolves its operand and, if the result is a
`KObject::KExpression`, dispatches the captured AST. Together they let user
code thread raw ASTs through eager-evaluating contexts (dict values, list
elements, function args) and thread `KExpression` values back through lazy
slots that would otherwise consume raw AST.

The sigils are **expression-level operators** in
[expression_tree.rs](../src/parse/expression_tree.rs), not entries in the
compound-operator registry. The parser keeps a `pending_sigil` flag while it
walks the input; consuming `#` or `$` sets the flag, and only the immediately
following `(` clears it by opening an `Expression` frame tagged with the head
keyword. On frame-close, the body is wrapped in an inner `Expression` part and
prepended with the head, producing the AST shape `(QUOTE <body>)` /
`(EVAL <body>)` that the QUOTE / EVAL builtins dispatch on. The
[QUOTE](../src/builtins/quote.rs) builtin's signature consumes a
`KExpression`-typed slot and returns the captured AST as a value;
[EVAL](../src/builtins/eval.rs)'s slot is `Any` so the scheduler
eagerly evaluates the operand first, after which the body checks the result is
a `KExpression` and tail-dispatches the inner AST in a fresh `CallArena`
(mirroring `MATCH`'s per-call frame so free names resolve against the
surrounding lexical scope but body-introduced bindings don't leak). EVAL
returns whatever the inner AST evaluates to; a non-`KExpression` operand
produces a structured `TypeMismatch`.

From the user's point of view, two surface forms are available. On its own
line — whether top-level or as the body of an indent-introduced block — `#expr`
and `$expr` work, with the operand running to end-of-line: `LET x =\n  #3`
binds `x` to the quoted AST of `3`. Inside a comma-continuation or a
bracket/dict-continuation, the bare form is unavailable and the user must
write `#(expr)` / `$(expr)` explicitly; a bare `#sym` in those contexts
errors. The asymmetry follows from where line-collapse runs: a sigil at the
head of an indent-led continuation gets wrapped to `<sigil>(<rest>)` before
the parser sees it, while comma- and bracket-continuation lines are appended
verbatim with no rewrite, so the bare sigil reaches the parser unchanged.
Tests lock both halves of the contract — explicit `#(2)` works in every
continuation form, bare `#2` works only under indent.

At the `build_tree` layer the rule is uniformly paren-only: any character
following `#` or `$` other than `(` is a parse error
(`expected '(' after '#', found <c>`), which is why the indent-collapse
rewrite in [whitespace.rs](../src/parse/whitespace.rs) is what makes the
bare-line surface possible. The bare `QUOTE` and `EVAL` keyword forms that
the desugaring produces happen to dispatch (the parser classifies all-caps
tokens as keywords, and the dispatch table matches), but they are not
documented surface — user code goes through the sigils.

## Open work

- Source spans on `KExpression`
  ([Error-handling surface follow-ups](../roadmap/error-handling.md)) — error
  frames currently can't point to a line/column in source.
