# Recursion over runtime data

**Problem.** The walks that follow a value's structure recurse once per level
of it: [rendering](../../src/values/render.rs), [equality](../../src/values/equality.rs)
and the [crossing](../../src/values/crossing.rs)'s deep copy in `values`. A
program builds that structure at run time, without bound, so a deep enough
value overflows the interpreter's stack rather than raising a koan error. Under
a debug build on a test thread, rendering or comparing a chain of newtypes over
records, each naming the next, aborts at about a thousand levels, and copying
it at about three thousand. The copy also recurses through a knot member's
family ([knot/copy.rs](../../src/knot/copy.rs)), which rebuilds each value the
knot holds through the same copy, so closures that capture closures nest the
recursion too. The same copy rebuilds a knot once per reference to it: two
members of one knot crossing together arrive as two copies of that knot.
[`memory::strongly_connected_components`](../../src/memory/components.rs) is
the precedent: it recursed per chain link, overflowed at load on about a
thousand chained `LET`s, and now walks an explicit worklist.

Source nesting has the same failure one layer down. The
[`sexlex`](../../sexlex/README.md) reader recurses per group, and every walk
over the parsed syntax recurses per nested part, so `PRINT [[[…]]]` or a quote
`#((((…))))` ten thousand deep crashes the loader, and so does a dotted chain
`r.a.a.a…` a hundred thousand long, which nests the syntax with no group at all.
A long operator run, `1 + 1 + …` with ten thousand operands, parses flat and
crashes the load too: the [operator-run rewrite](../../src/scope/shape/build/rewrite.rs)
folds it into nodes nested once per operator, which the shape builder then walks
recursively.

**Acceptance criteria.**

- Tests on the default test thread build a chain of newtypes over records, each
  naming the next, a hundred thousand deep, and render it, compare it with
  `==`, and cross it into another region under a copy, each without
  overflowing.
- Every walk in `values` that follows a value's structure runs over an explicit
  worklist, so the stack it uses does not grow with the value's depth. A knot
  member's family lists the values its knot holds, and rebuilds the knot through
  a callback that answers each of them with its finished copy, so the copy's
  worklist owns the knot's contents.
- Two references to one knot crossing in one placement arrive as members of
  one copy of that knot: a copied module's two members of one function knot
  capture each other, and values copied together through `copy_severed`, as a
  call's birth copies its callee and arguments, share one copy of a knot they
  both reach.
- A program whose syntax nests deeper than the parser's limit is refused at
  load with a parse error naming the limit, never a crash. The limit is one
  constant, and it counts groups as `sexlex` reads them and the nesting of the
  lowered syntax, dotted chains included. An operator run counts as the nesting
  its rewrite folds it into, so a run too long for the limit is refused at load
  like any other too-deep syntax.
- The interpreter runs a program's load and run on a stack of a fixed size, and
  a test on a stack of that size loads and runs, in a debug build, a program
  nested to the limit in each nesting shape: parentheses, a quote compared with
  `==` and printed, a list literal, a record literal, a dotted chain, a folded
  operator run, and a pairwise operator run whose operands are hoisted and whose
  pairs are `!=`.

**Directions.**

- *An explicit worklist per walker — decided.* Each walker keeps its own stack
  in scratch, as `memory::strongly_connected_components` does, rather than a
  shared traversal framework. Rendering and equality stage in the scratch they
  are handed; the copy's two doors make a bump of their own, since no step
  scratch reaches the birth crossing.
- *The type lattice — deferred* to
  [recursion over run-time types](recursion-over-run-time-types.md). Every
  lattice walk a value's carried type reaches recurses too, but converting them
  is a redesign of the lattice's relation machinery of its own. The chain these
  tests build is nominal, so its carried type stays one node deep and no
  lattice walk here grows with it.
- *Quoted syntax — decided.* Syntax gets a depth limit rather than worklists.
  The parser refuses too-deep source here, and
  [code splicing](code-splicing.md) refuses too-deep composed code with an
  error value. A quote's syntax comparison in equality stays recursive under
  that limit.
- *Operator runs — decided.* A run counts toward the one depth limit at parse,
  as the nesting its rewrite will build, rather than under a cap on run length
  of its own. That nesting adds to the nesting around the run, and the shape
  builder walks a nested body while the enclosing statement's walk is still on
  the stack, so only a count over the whole parsed expression bounds it.
- *A knot family's rebuild — decided.* The family lists its knot's values and
  keeps rebuilding through a copy callback, which the copy answers from a
  cursor over the finished copies, checking each value asked for against the
  list. Listing and rebuilding stay two walks: the knot is tied once, after
  every value it holds has a copy.
- *One copy per knot per placement — decided.* A copy keys each knot it has
  rebuilt by one of its members, and a placement copying several values, a
  call's birth among them, runs them through one copy. Plain values reached
  twice still copy twice, and a value's weight still counts a knot once per
  reference to it, since a memoized weight cannot see sharing below it.

## Dependencies

**Requires:**

- [`values`](../../src/values/README.md) — shipped: the walks this item turns
  into worklists.

**Unblocks:**

- [Code splicing](code-splicing.md) — the syntax depth limit composed code is checked against.
