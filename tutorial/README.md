# Koan tutorial

A guided tour of the Koan language for people who want to *write* Koan. It works
through the language one feature at a time, with runnable examples throughout.
Every code block has been run against the interpreter, and the expected output
is shown beneath it. No knowledge of how the runtime is built is assumed or
needed.

If you just want to run something, jump to
[Getting started](01-getting-started.md). Otherwise, read on for the shape of
the language before diving into the chapters.

## The big picture

A Koan program is a sequence of **expressions**, run top to bottom. There are no
statements and no loops — an expression always produces a value.

What an expression *does* is decided by its **shape**. A shape is its fixed
keywords (uppercase words like `LET`, `PRINT`, `MATCH`) plus its typed slots (the
values and names filling the gaps between them). Koan matches that shape against
every function in scope and runs the best match. Keywords are not names you can
call or pass around — they're fixed markers that only mean something as part of a
shape. Lowercase **identifiers** name values; capitalized **type names** name
types.

You extend the language by defining functions with `FN`, which simply registers
a new shape. Built-in forms and your own functions work the same way, so the
language grows from the inside. Types come in two flavors you'll declare
yourself: **tagged unions** (a value that is one of several alternatives) and
**newtypes** (a fresh identity over a representation, including records with
named fields).

## Chapters

Read them in order the first time through — each builds on the last.

1. [Getting started](01-getting-started.md) — running a program, expressions,
   grouping by parentheses and indentation.
2. [Values and types](02-values-and-types.md) — numbers, strings, lists,
   dictionaries, and the vocabulary for naming types.
3. [Names, binding, and dispatch](03-names-and-dispatch.md) — `LET`, lexical
   scope, the three token classes, and how matching by shape works.
4. [Functions](04-functions.md) — defining and calling, return types,
   overloading, anonymous functions, named arguments, closures.
5. [Tagged unions](05-tagged-unions.md) — sum types: values that are one of
   several tagged alternatives.
6. [Pattern matching](06-pattern-matching.md) — `MATCH`, unwrapping a union, and
   the recursion idiom.
7. [Records](07-records.md) — product types with named fields, field access, and
   record projection.
8. [Newtypes](08-newtypes.md) — fresh nominal identities over a representation,
   and mutually recursive types.
9. [Errors](09-errors.md) — error values, catching with `TRY`, capturing with
   `CATCH`, and `Result`.
10. [Quoting and evaluating](10-quoting.md) — capturing an expression as data
    with `#` and running it with `$`.
11. [Modules](11-modules.md) — grouping bindings, signatures, and ascription.
12. [Functors](12-functors.md) — modules parameterized by modules, and signature
    specialization.

A condensed [surface reference](reference.md) lists every form on one page once
you know your way around.

## What isn't in the language yet

Koan is young, and some things you might reach for aren't here:

- **No arithmetic, comparison, or logical operators.** `1 + 1` does not add.
  Computation is expressed through functions and dispatch.
- **No loops.** Recursion is the iteration model (see
  [Pattern matching](06-pattern-matching.md)).
- **No comments.** `#` is the [quoting](10-quoting.md) sigil, not a comment
  marker.
- **No user-declared traits or interfaces** beyond what unions, records, and the
  module system provide.

When a snippet doesn't behave the way you expect, one of these is a likely cause.
