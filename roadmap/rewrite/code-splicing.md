# Code splicing

Code taken apart and put together at run time, and the shape such code runs
in, laid down where the code lives.

**Problem.** No koan operation takes a fragment out of a quote or splices code
into one, so every code value is a quote the program wrote. An `EVAL` builds a
block shape for its code each time it runs, and `Shape::for_eval`
([scope/shape.rs](../../src/scope/shape.rs)) lays it down in program storage,
which is never released during a run, so an `EVAL` evaluated in a loop grows it
without bound.

**Acceptance criteria.**

- A fragment taken out of a quote holds each fellow knot member it names as
  that member's value word, and no code value outside a knot holds an edge.
- A block value carries its shape, laid down where the value lives rather than
  in program storage, so an `EVAL` evaluated in a loop does not grow program
  storage.

**Directions.**

- *Taking code apart — decided.* A quote born in a knot holds edges among its
  bindings, and an edge means nothing outside its knot. A fragment taken out of
  one resolves each edge through its source member, as a nested function
  captures a fellow member as a value
  ([closure bindings and edges](../../src/knot/README.md#closure-bindings-and-edges)).
  So only a quote node is knot-aware; a fragment naming a member weighs its
  whole knot, and a crossing copies that knot whole.
- *Splicing one part or many — open.* A splice into a quote puts in one part or
  a run of parts. Lisp spells the two `,x` and `,@xs`, Julia `$x` and
  `$(xs...)`, and Rust's `quote!` adds a separator with `#(#xs),*`. Koan could
  instead let the spliced code's kind decide, at the risk of a list of
  expressions meant as one literal part. Taking a fragment out of a quote and
  splicing one in are code's slice and splice, as they are a list's
  ([slicing and splicing](slicing-and-splicing.md)).
- *A kind for built code — open.* A block value carrying its shape is code and
  a shape. It is typed by its syntax's kind, or by a kind of its own that an
  explicit `BUILD` returns, beside the syntactic kinds
  ([the code family](../../src/type_lattice/README.md#the-code-family)).

## Dependencies

**Requires:**

- [Quotes resolve where they are written](eval-scope.md) — a quote's bindings and knot edges.
- [Slicing and splicing](slicing-and-splicing.md) — slice and splice as builtins, and views.

**Unblocks:**

- [Code names](code-names.md) — a shape built at run time for code the builder cannot trace.
