# Code splicing

Code taken apart and put together, and the shape such code runs in, laid down
where the code lives.

**Problem.** No koan operation takes a fragment out of a quote or splices code
into one, so every code value is a quote the program wrote. An `EVAL` builds a
block shape for its code each time it runs, and `Shape::for_eval`
([scope/shape.rs](../../src/scope/shape.rs)) lays it down in program storage,
which is never released during a run, so an `EVAL` evaluated in a loop grows it
without bound.

**Acceptance criteria.**

- With `stmts` bound to `[#(LET x = 4) #(PRINT x)]`, `EVAL #($..stmts)` prints
  `4`, and a `$` name in a spliced part keeps the binding its own quote gave it.
- `$..xs` makes the elements of `xs` the quote's syntax where the quote is
  written, and `..$xs` binds `xs` there and spreads its value when the code
  runs.
- A `$..` operand that is not a name, a `$..` element that is not code, and
  `\..xs` are refused.
- With `args` bound to `[#(a + b) #(c)]`, `#(f $..args)` is the quote of
  `f (a + b) c`.
- A block holding `$..stmts` between two statements splices the same
  statements whether it is written on one line or split across lines.
- A fragment taken out of a quote keeps its `$` bindings, and a hole a binder
  beside it filled is a hole again: `PRINT x` taken out of a block that also
  holds `LET x = 4` has `x` a hole.
- A fragment taken out of a quote holds each fellow knot member it names as
  that member's value word, and no code value outside a knot holds an edge.
- Code composed at run time has its shape built once per code value and laid
  down where the value lives rather than in program storage, so an `EVAL` of
  composed code evaluated in a loop does not grow program storage.

**Directions.**

- *Taking code apart — decided.* Taking a fragment out of a quote is code's
  slice, as it is a list's ([slicing and splicing](slicing-and-splicing.md)). A
  hole names no binding, so a fragment taken away from the binder that filled
  it holds a hole again, while a `$` name carries its binding and the `\` marks
  a fragment holds are recomputed into its carried type. A quote born in a knot
  holds edges among its bindings, and an edge means nothing outside its knot. A
  fragment taken out of one resolves each edge through its source member, as a
  nested function captures a fellow member as a value
  ([closure bindings and edges](../../src/knot/README.md#closure-bindings-and-edges)).
  So only a quote node is knot-aware; a fragment naming a member weighs its
  whole knot, and a crossing copies that knot whole.
- *Splicing where the quote is written or when the code runs — decided.* The
  outer sigil says when ([splicing](../../src/scope/README.md#splicing)).
  `$..xs` spreads where the quote is written, so the elements of `xs` become
  the quote's syntax when the quote is born; `..$xs` binds `xs` there and
  spreads its value when the code runs, as `..` does outside a quote. `$` still
  never evaluates, since splitting a list runs no koan code, so the operand is a
  name, and a computed list enters by `LET` first. The operand is a list, and
  each element must be code, or a code element would be spliced syntax or a
  nested quote according to its kind. `\..xs` has no reading: a `\` name binds
  where a body is built, to a slot whose value arrives with each call. `..` is
  one level deep, as Lisp's `,@xs` and Julia's `$(xs...)` are.
- *The level a splice acts at — decided.* It is read from the lowered parts,
  never from the lines, since a block is any node of two or more groups
  whether written on one line or several. A splice whose siblings other than
  splices are all groups acts at statement level, and each element adds its
  statements, since an `Expression` stands in for a `Block`. Any other acts at
  part level: each element is one part, bare when it is a single part and a
  group otherwise, as Julia's `$ex` is. A node of splices alone is at
  statement level, so an expression cannot be spliced wholly from parts.
- *A splice alone on a line — decided.* Lowering, where a layout line is still
  told apart from a written paren, makes a layout line that is one splice or
  spread atom that atom's part rather than a statement holding it, beside the
  rule that a line reading `a.b` is that call
  ([lowering](../../src/parse/README.md#the-division-of-labour-with-sexlex)).
  A written `($..xs)` stays a group, as every written paren does.
- *A kind for built code — decided.* An `EVAL`'s shape depends on its code
  alone: holes bind to binders in the code and to the builtin table, `$` names
  carry their bindings, and `\` marks are the shape's parameters, which the
  `EVAL` supplies. A written quote's is built where the program loads
  ([building code](../../src/scope/README.md#building-code)); composed code's is built once and kept where
  the code value lives, as invisible as a view, and code keeps its syntactic kind
  ([the code family](../../src/type_lattice/README.md#the-code-family)). Code
  becomes a callable as the body of an `FN` ([quotes and functions](../../src/scope/README.md#quotes-and-functions)).

## Dependencies

**Requires:**

- [Slicing and splicing](slicing-and-splicing.md) — slice as a builtin, views, and `..` outside a quote.

**Unblocks:**

- [Code names](code-names.md) — a shape built at run time for code the builder cannot trace.
