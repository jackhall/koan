# Code as values

Quoted code typed by what it is — a literal, a symbol, an expression, a block
or an arm — named in a channel of its own, and the shape builder's own
representation of it.

**Problem.** Every quote and every raw group is typed `KExpression`, and a raw
identifier `Identifier` ([values/admission.rs](../../src/values/admission.rs)),
so no type tells a symbol from an expression or a block. The same raw-part
types are also how a
[builtin shape](../../src/parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact)
says what to capture raw: `lazy_kinds_at` keeps a `(…)` group raw because its
slot is typed `KExpression`, and a table law pins every `Body`, `Branches`,
`Quantifiers` and `Data` slot to it. So no builtin slot can say "evaluate this
and require code": `EVAL`'s operand is typed `Any`
([builtin_shapes.rs](../../src/parse/builtin_shapes.rs)) and checked for a
quote when it runs. `UNARY OP <symbol> OVER <operand>` types both its slots
`Any` and keeps neither raw, while its `…Returning` twin keeps its symbol raw.
A builtin's raw parts — a `FN` or `EXPR` body, an `EXPR`'s signature, an arm —
are written unquoted, while a user function's code argument is quoted at the
call, so a reader cannot tell from a builtin's call which of its parts run.
`ATTR`'s label is typed `NameToken | Str`, so a field named at run time is named
by a string. Code has no name class of its own: a module exports it only as a
value. And `Shape::for_eval` lays a fresh block shape down in program storage,
which is never released during a run, so an `EVAL` evaluated in a loop grows it
without bound.

**Acceptance criteria.**

- Five kinds of code lie under `Code`: literal, symbol, expression, block
  and arm.
- A quote is typed by its body as written: `#(y)` is a symbol, `#(42)` and
  `#("y")` are literals, and `#((y))` and `#(f x)` are expressions.
- No slot captures its part raw: every slot evaluates its part, and a part meant
  as code is written as a quote, a `FN` or `EXPR` body and an `EXPR`'s signature
  included. A binder name is written bare.
- A quote in a callable's body slot is built as that callable's body shape.
- The tutorial's snippets write quoted bodies and signatures.
- `ATTR`'s label is a symbol. `ATTR p y` and `LET which = #(y)` followed by
  `ATTR p (which)` read the same field, a `Str` label is a no-overload miss, and
  tutorial 07's field read named at run time is spelled this way.
- `EVAL`'s operand is typed `Code`, so `$(n)` over a number is a no-overload
  miss rather than a check of its own.
- An arm is a guard type and a block binding `it`. The scope builder builds each
  `MATCH` and `TRY` arm as one, and reads from it that its block's last
  statement is in tail position wherever the `MATCH` or `TRY` is, that `it` is
  narrowed to the guard, and that a `TRY` body's error goes to its arms rather
  than to the enclosing frame.
- A block value carries its shape, laid down where the value lives rather than
  in program storage, so an `EVAL` evaluated in a loop does not grow program
  storage.
- A sigiled name holds only code and resolves to a coordinate where the shape is
  built; a sigiled parameter binds per call.
- A shape is built from code only where it traces that code back to written
  quotes, as `USING … SCOPE` traces its operand; code it cannot trace is shaped
  when it is evaluated.
- A `SIG` declares a code member with `VAL`, as a slot, or with `LET`, as
  manifest code.
- A fragment taken out of a quote holds each fellow knot member it names as that
  member's value word, and no code value outside a knot holds an edge.
- Code equality follows bindings as a bisimulation: a quote equals its copy,
  `LET echo = #(PRINT echo)` compared with a copy of itself terminates equal,
  and two quotes of the same text whose names bind different values are
  unequal.
- Printing code never follows a binding.

**Directions.**

- *Metaprogramming model — decided.* Hygienic fexprs: a callee takes code
  through a slot typed by a code kind, the caller quotes it, and the quote's
  names resolve where it is written
  ([quotes resolve where they are written](eval-scope.md)). Koan has no
  expansion system and no global execution phases. Rewriting stays the shape
  builder's own, as the pairwise rewrite is, since a user's rewrite rule acts at
  a distance; and code generated per argument type, as Julia's `@generated`
  does, solves too narrow a problem.
- *Raw operands are quoted at the call — decided.* Every raw operand is visible
  where it is written, a builtin's included. A binder name is special and stays
  bare.
- *Arms and the other raw parts — open.* Whether a `MATCH` or `TRY` arm is
  quoted, and each other part a builtin reads raw: an `OP` body, a `MODULE`,
  `GROUP` or `SIG` body, a `USING … SCOPE` body. Recommended: quote arms.
- *One representation — decided.* The code kinds are the shape builder's own
  categories, not a reflective model beside them. Code is built through
  `parse`'s node constructor, whose anonymous-slot naming gives a fresh name —
  the doors the [pairwise rewrite](../../src/scope/README.md#operator-groups)
  and the [nested-binder hoist](nested-binders.md) use — so a block value's
  shape is the shape the builder builds.
- *How the kinds are ordered — open.* Disjoint, or a symbol and a literal each
  also an expression of one part, so an expression slot admits them.
- *The binder-slot raw-part types — open.* Whether `NameToken`,
  `TypeNameToken`, `SigiledTypeExpr` and `RecordType` become code kinds or stay
  beside them. Recommended: they become code kinds, so one family types every
  part the shape builder reads raw.
- *A code naming channel — decided.* A name class of its own, marked by a sigil
  since case is spent, holding only code. A sigiled parameter binds per call as
  a type parameter does: a quote's shape travels with its value, so running
  code needs nothing from the shape that runs it, and only building a new shape
  from code needs the code where the shape is built. Elaboration may evaluate
  code, as a subdispatch does, provided that code is deterministic, since what
  elaboration builds is interned. A `VAL` code member is sealed as a value is
  behind its type: a holder can run the code but not see which code it is.
- *The sigil's character — open.* `#` reads as code, but `#y` sits one pair of
  parens from `#(y)`, the quote of the symbol `y`.
- *What a sigiled name resolves to — open.* Narrowing the family alone is
  admission's job, so the sigil needs a resolution rule of its own. One
  candidate: inside a quote, a sigiled name splices its code, while a lowercase
  name stays a captured reference.
- *Splicing one part or many — open.* A splice into a quote puts in one part or
  a run of parts. Lisp spells the two `,x` and `,@xs`, Julia `$x` and
  `$(xs...)`, and Rust's `quote!` adds a separator with `#(#xs),*`. Koan could
  instead let the spliced code's kind decide, at the risk of a list of
  expressions meant as one literal part. Taking a fragment out of a quote and
  splicing one in are code's slice and splice, as they are a list's
  ([slicing and splicing](slicing-and-splicing.md)).
- *A code binder in a module's self-signature — open.* A slot at its code kind,
  as a value binder is, or a manifest member, as a type binder is.
- *Manifest code in a `SIG` — open.* Two textually identical `SIG`s are one
  type, so manifest code that captures from its surroundings would make
  identical text differ. Code equality follows bindings, so either manifest code
  captures nothing, or a `SIG`'s identity compares its code by something other
  than equality.
- *Taking code apart — decided.* A quote born in a knot holds edges among its
  bindings, and an edge means nothing outside its knot. A fragment taken out of
  one resolves each edge through its source member, as a nested function
  captures a fellow member as a value
  ([closure bindings and edges](../../src/knot/README.md#closure-bindings-and-edges)).
  So only a quote node is knot-aware; a fragment naming a member weighs its
  whole knot, and a crossing copies that knot whole.
- *Code equality — decided.* Code compares by structure and follows its
  bindings as a bisimulation, as circular data does
  ([equality](../../src/values/README.md#equality-and-rendering)). Ignoring
  bindings would make two quotes that evaluate differently equal, and comparing
  them by node identity would make a copy unequal to its source.
- *A binding that holds a function — open.* A function compares `Incomparable`
  ([equality and rendering](../../src/knot/README.md#equality-and-rendering)),
  and every called name and keyworded use binds one, so a bisimulation reaching
  a function makes nearly all code incomparable. The alternatives: accept that;
  pair function nodes by body shape and bisimilar captures, giving functions a
  structural equality inside code only; or compare a function binding by node
  identity, so code that calls a function is unequal to its copy.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Dispatch](dispatch.md) — `ATTR`'s symbol label, `EVAL`'s operand, and code-typed parameters.
- [Quotes resolve where they are written](eval-scope.md) — the code representation a quote's bindings ride in.
