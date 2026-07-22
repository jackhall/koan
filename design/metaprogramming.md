# Metaprogramming

Koan's metaprogramming model is quotation plus splicing: a quote captures code
as an ordinary value, and `EVAL` runs such a value exactly as if its content
were written at the `EVAL` site. There is no macro layer, no staging
annotations, and no separate template language — the two sigils, the ordinary
value system, and one scheduling rule (the EVAL barrier) carry the whole story.

[expressions-and-parsing.md](expressions-and-parsing.md) owns the parse-level
mechanics of the `#` / `$` sigils and of lazy slots;
[operators.md](operators.md) owns the operator declaration surface. This doc
owns the semantics that make *assembled* code executable: what an expression
value is, how the two literal spellings of an expression argument relate, which
arguments must be literal, how `EVAL` splices, and how the scheduler keeps
concurrent siblings deterministic around a splice.

## Terms

- **Expression value** — a
  [`KObject::KExpression`](../src/machine/model/values.rs): a koan AST held as
  an ordinary first-class value. It can be bound with `LET`, passed to and
  returned from functions, and stored in containers like any other value.
- **Quote** — `#(…)`: a parse-static
  [`ExpressionPart::QuotedExpression`](../src/machine/model/ast.rs). It
  evaluates to the expression value of its body; the body itself never
  dispatches. Quotation happens entirely at parse time — there is no run-time
  quoting operation.
- **Literal expression part** — an expression whose content appears textually
  in place: either a plain parenthesized group `(…)`
  (`ExpressionPart::Expression`) or a quote `#(…)`
  (`ExpressionPart::QuotedExpression`). Both are parse-static: their content is
  readable from the unevaluated AST, before anything runs.
- **Dynamic expression argument** — an expression value that reaches a slot any
  other way: a name bound to a quote, a call returning an expression value, any
  computed sub-expression. Its content does not exist until evaluation, so
  nothing about it is readable parse-statically.
- **Lazy `:KExpression` slot** — a builtin parameter typed `:KExpression`. A
  literal expression part in such a slot is captured **raw** — handed to the
  builtin un-dispatched
  ([`pick.rs`](../src/machine/core/kfunction/pick.rs)) — so the builtin decides
  what its content means. See
  [expressions-and-parsing.md § Lazy slots](expressions-and-parsing.md#lazy-slots).
- **Shape slot** — a lazy `:KExpression` slot whose content a declaration
  surface reads as *shape data*, never runs: `FN`'s signature, `OP`'s symbol,
  and `GROUP`'s pairwise combiner symbol. (An `FN` or `OP` *body* slot is lazy
  too, but its content is code the declaration will eventually run — not a
  shape slot.)
- **Block** — a parenthesized body holding a sequence of top-level expressions:
  a module body, a `GROUP` body, an `FN` body, or a submitted top-level block.
  A block's top-level expressions evaluate concurrently by default, subject to
  the scheduler's dependency edges
  ([execution/README.md](execution/README.md)).
- **Splice** — what `EVAL` does: evaluate its operand to an expression value,
  then evaluate that value's AST at the `EVAL` site as if it were written
  there.
- **EVAL barrier** — the scheduling rule that sequences a block around a
  splice, defined below.

## The two literal spellings are one spelling

In a lazy `:KExpression` slot, `#(…)` and `(…)` are interchangeable. Both are
slots for dispatch purposes (so they compute the same untyped key), both are
captured raw, and both resolve to the same expression value. The quote adds no
meaning where evaluation is already suppressed.

The quote *matters* only in eager position — anywhere an unquoted expression
would evaluate. `LET ast = #(1 + 2)` binds the AST of `1 + 2`; `LET three =
(1 + 2)` binds `3`. That is the quote's entire job: suppress evaluation where
evaluation is the default.

Consequently every declaration surface accepts both spellings uniformly:

```
OP #(+) OVER :(LIST OF Number) = (…)      -- identical to the line below
OP (+) OVER :(LIST OF Number) = (…)

FN (ADD left :Number right :Number) = (…) -- identical to the line below
FN #(ADD left :Number right :Number) = (…)

GROUP num_compare PAIRWISE FOLD (BOTH) LEFT = (…)   -- combiner slot likewise
```

The rule this imposes on the implementation: every reader of a shape slot is
**kind-blind** across the two literal part kinds. Each shape slot has exactly
one reader, shared verbatim by every consumer of that slot — the dispatch-time
slot read, the parse-static binder-bucket read that seeds the pending-overload
park edges installed at statement submission, and `GROUP`'s member scan. All consumers reach the same verdict on the
same declaration; none discards a diagnostic another one surfaces. A
declaration therefore either fully registers (bodies, registry entry, park
edges, group membership) or is fully refused — never a partial state whose
visibility depends on scheduler order.

Note what interchangeability does *not* mean: an unparenthesized symbol is not
a spelling of anything. `OP + OVER Number` puts a `Keyword` part where the slot
goes, which changes the expression's untyped key — that spelling keys a bucket
no fixed overload matches and is simply not an `OP` declaration. The
parentheses (with or without the `#`) are what make the symbol an argument.

## Shape slots are parse-static

A shape slot requires a **literal** expression part. A dynamic expression
argument in a shape slot — `FN sig = (…)` with `sig` bound to an assembled
signature, `OP sym OVER …` with `sym` computed — is refused at the declaration
with a diagnostic naming the `EVAL` route.

The reason is staging, not spelling. Two consumers read a declaration's shape
*before it evaluates*:

- the **parse-static binder install** seeds park edges at statement submission,
  so a sibling expression using the declared name parks until the declaration
  finalizes instead of racing the scheduler
  ([operators.md § Visibility](operators.md#visibility));
- **`GROUP`'s member scan** collects member symbols from the unevaluated body
  and registers the member-set record before a single body expression runs
  ([operators.md § Groups](operators.md#groups)).

A shape that exists only after evaluation has no parse-static reading, so a
declaration built around one could register bodies while installing no park
edges — the partial state the one-reader rule forbids. Runtime-assembled
declarations are not second-class, though: they go through `EVAL`, whose
barrier restores exactly the determinism the park edges provide for literal
declarations.

## EVAL splices in place

`EVAL <operand>` — surface form `$(…)`, see
[expressions-and-parsing.md § Quote and eval sigils](expressions-and-parsing.md#quote-and-eval-sigils)
— evaluates its operand eagerly to an expression value, then evaluates that
value's AST **at the EVAL site**:

- **Same scope.** Free names in the spliced AST resolve against the scope
  enclosing the `EVAL`, with the ordinary lexical rules.
- **Declarations land at statement positions.** A `LET`, `FN`, `OP`, or any
  other binding form at a statement position of the spliced AST registers in the
  enclosing scope exactly as if hand-written at the site — bindings,
  function-bucket overloads, and operator-registry entries included. A binder in
  an eagerly evaluated value position of the spliced AST is the same structured
  `NestedBinder` error as hand-written source in that position (see
  [execution/name-placeholders.md](execution/name-placeholders.md)): the position
  rule does not distinguish spliced code from written code.
- **Same result.** The `EVAL` expression evaluates to whatever the spliced AST
  evaluates to. A non-`KExpression` operand is a structured `TypeMismatch`.

`EVAL` behaves this way **everywhere**. Koan has no statement/expression
distinction, and `EVAL` itself has no position-dependent semantics: at the top
level of a module body, inside an `FN` body, inside a `GROUP` body — always a
splice, always the enclosing scope. The position rule applies to the spliced
*content*, not to `EVAL`: a splice at a statement position lands its
declarations, and a splice in an eagerly evaluated value position rejects a
binder in its content exactly as hand-written source there would. So `EVAL q` is
exactly as powerful as writing `q`'s content in place, shadowing included — both
sides accept and reject alike; treat an `EVAL` over an AST you didn't assemble
with the same care as code you didn't write.

## The EVAL barrier

A block's top-level expressions evaluate concurrently, and a splice can bind
names its siblings use — left unordered, a sibling's view of the splice would
depend on scheduler order. The barrier removes that race:

**Any block-level expression containing an `(EVAL …)` head anywhere in its
tree is a barrier: every later expression in the same block parks on it.**

- Detection is parse-static. Both the `EVAL` keyword and the `$` sigil produce
  the head-tagged `(EVAL <body>)` AST shape
  ([expressions-and-parsing.md](expressions-and-parsing.md#quote-and-eval-sigils)),
  so submission can mark barriers without evaluating anything.
- The mechanism is ordinary dependency edges installed at submission time —
  each later sibling gains a dep on the barrier expression. No placeholder
  registry entries, no wildcard park keys: the park target is a known node.
- An `EVAL` nested inside a sub-expression hoists its barrier to the containing
  block-level expression — the whole containing expression is the barrier.
- An `FN` body is its own block; a barrier inside it sequences the remainder of
  that body per call and touches nothing outside.

The resulting visibility rule is the lexical-cutoff rule with teeth:
expressions **before** the barrier never see the splice's bindings; expressions
**after** it always do. Within one block, order around an `EVAL` is
significant *by design* — that is the sequencing the splice's flexibility
costs, and only blocks that contain an `EVAL` pay it. An `EVAL` used purely
for its value pays the same barrier; uniformity is worth the occasional
over-sequencing.

## Groups and spliced members

An `EVAL` at the top level of a `GROUP` body whose splice declares an `OP` adds
that operator to the group **when the EVAL finalizes**: the group's one shared
record extends its member set, and the subsets covering the new member register
into the group's child scope then. The barrier makes the timing deterministic:

- Body expressions sequenced **before** the `EVAL` never see the member; body
  expressions **after** it always do. The group-body invariant is therefore:
  declaration order inside the body does not matter, *except across an EVAL
  barrier* — where sequencing is the point.
- A `USING` window opens a group only after its body completes, so an external
  observer always sees the finished member set.
- Mode consistency, operand typing, and the pairwise-only rule for
  heterogeneous members apply to a spliced `OP` exactly as to a written one.
- The scope rule from [operators.md](operators.md#groups) is unchanged: a
  spliced `OP` joins the group because it lands at the body's top level, while
  an `OP` spliced inside an `FN` in the body still declares in that `FN`'s
  per-call scope and joins no group. Membership follows the scope the
  declaration lands in, and splicing never moves scopes.

## Assembling code

The intended workflow, end to end: quote the pieces, combine them with
ordinary functions into a full declaration's AST, then splice it.

```
LET body = #(left + right + 1)
LET declaration = (make_op {symbol = #(⊕), operator_body = body})
EVAL declaration                    -- ⊕ now declared in this scope
LET sum = (2 ⊕ 3 ⊕ 4)               -- sequenced after the barrier; sees ⊕
```

(`make_op` here is an ordinary captured function returning an expression
value, called by named record as any captured function is.)

The shape-slot rule and the splice rule divide the labor: a *literal*
declaration is read parse-statically and parallelizes freely; an *assembled*
declaration is spliced and sequences its block. Nothing in between — a
declaration with a dynamic shape part — exists, because in-between forms are
exactly the ones whose behavior would depend on scheduler order.

## Open work

The machinery this doc specifies beyond today's builtins is tracked in
[roadmap/metaprogramming/](../roadmap/metaprogramming/README.md):

- [One kind-blind reader per shape slot](../roadmap/metaprogramming/one-reader-per-shape-slot.md)
  — spelling interchangeability and the all-or-nothing registration rule for
  `FN` / `OP` / `GROUP` declarations.
- [EVAL splices in place](../roadmap/metaprogramming/eval-splices-in-place.md)
  — the splice semantics and the block barrier.
- [Group members may arrive by splice](../roadmap/metaprogramming/group-members-by-splice.md)
  — late member join for `GROUP`.
