# Code as values

Quoted code typed by what it is — a literal, a symbol, an expression, a block
or a set of arms — and written as a quote wherever a builtin takes code.

**Problem.** Every quote and every raw group is typed `KExpression`, and a raw
identifier `Identifier` ([values/admission.rs](../../src/values/admission.rs)),
so no type tells a symbol from an expression or a block. The raw-part types
`Identifier`, `NameToken`, `TypeNameToken`, `KExpression`, `SigiledTypeExpr`
and `RecordType` are unordered, each only under `Code`. They are also how a
[builtin shape](../../src/parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact)
says what to capture raw: `lazy_kinds_at` keeps a `(…)` group raw because its
slot is typed `KExpression`, and a table law pins every `Body`, `Branches`,
`Quantifiers` and `Data` slot to it. So no builtin slot can say "evaluate this
and require code": `EVAL`'s operand is typed `Any`
([builtin_shapes.rs](../../src/parse/builtin_shapes.rs)) and checked for a
quote when it runs. `UNARY OP <symbol> OVER <operand>` types both its slots
`Any` and keeps neither raw, while its `…Returning` twin keeps its symbol raw.
A builtin's raw parts — a `FN` or `EXPR` body, an `EXPR`'s head, a `MATCH` or
`TRY` arm — are written unquoted, while a user function's code argument is
quoted at the call, so a reader cannot tell from a builtin's call which of its
parts run. `ATTR`'s label is typed `NameToken | Str`, so a field named at run
time is named by a string. And the parser peels every redundant paren, so
`#((y))` and `#(y)` are the same quote.

**Acceptance criteria.**

- The code family is ordered: a value name (`Identifier`) and a type name
  (`TypeNameToken`) lie under `Symbol`; `Literal`, `Symbol`, a quoted `:(…)`
  (`SigiledTypeExpr`), a quoted `:{…}` (`RecordType`) and `Arms` lie under
  `Expression`; `Expression` lies under `Block`, and `Block` under `Code`. The
  type spelled `KExpression` is spelled `Expression`, and `NameToken` is
  spelled `Symbol`.
- A quote is typed by its body as written: `#(y)` is a symbol, `#(42)` and
  `#("y")` are literals, `#((y))` and `#(f x)` are expressions, a quote of two
  or more statements is a block, and `#{Some: (…)}` is arms. A parenthesis the
  program writes is never peeled.
- A callable's body, an `EXPR`'s head and a `MATCH` or `TRY` arm set are
  written as quotes, and the shape builder builds each where it is written: a
  body as that callable's body shape, a head as its parameters, each arm as a
  block. A bare group or a name in one of those positions is refused where the
  shape is built.
- A binder name, a label, a type expression, a definition (`SIG`, `UNION`,
  `NEWTYPE`) and a body that runs where it is written (`MODULE`, `GROUP`,
  `USING … SCOPE`, `TRY`, `CATCH`) are written bare; every other slot evaluates
  its part, and no slot keeps a part raw.
- The tutorial's snippets write quoted bodies, heads and arms.
- `ATTR`'s label is a symbol: a bare name is the label itself and any other
  part is evaluated, so `ATTR p y` names the field `y` and `ATTR p (which)`
  reads `which`. No overload takes a `Str` label, and tutorial 07's field read
  named at run time is spelled `LET which = #(y)`.
- `EVAL`'s operand is typed `Code`.
- Arms are written `#{<guard>: <body>, …}`, each entry a guard and a block
  binding `it`, and a `_` key is the default arm. The scope builder builds each
  `MATCH` and `TRY` arm as one, and records its guard as the type of `it` and
  that its block's last statement is in tail position exactly where the `MATCH`
  or `TRY` is.

**Directions.**

- *Metaprogramming model — decided.* Hygienic fexprs: a callee takes code
  through a slot typed by a code kind, the caller quotes it, and the quote's
  names resolve where it is written
  ([quotes resolve where they are written](eval-scope.md)). Koan has no
  expansion system and no global execution phases. Rewriting stays the shape
  builder's own, as the pairwise rewrite is, since a user's rewrite rule acts at
  a distance; and code generated per argument type, as Julia's `@generated`
  does, solves too narrow a problem.
- *What is quoted — decided.* A part that runs later, conditionally or never
  is quoted: a callable's body, an `EXPR` head, an arm set. A bare part runs
  where it is written, once, or declares — so a reader tells from a builtin's
  call which of its parts run. A binder name is special and stays bare.
- *Arms are a quoted dict — decided.* Guards map to blocks, `#{Some: (…),
  None: (…)}`. Selection is by specificity, as dispatch selects a candidate
  ([control expression shapes and errors](control-and-errors.md)), so written
  order does not matter and a dict's reading is honest.
- *`_` names a dict's default — decided.* Every dict literal may write `_` as
  a key; in an arm set it is the default arm. A value dict's default is
  [dict defaults](dict-defaults.md)'s, and until it ships a value dict holding
  `_` is refused where its shape is built.
- *One representation — decided.* The code kinds are the shape builder's own
  categories, not a reflective model beside them. Code is built through
  `parse`'s node constructor — the door the
  [pairwise rewrite](../../src/scope/README.md#operator-groups) uses — and its
  anonymous-slot naming gives a fresh name.
- *A block is syntax — decided.* A block is two or more statements, so a
  quote's type is fixed where it is written. Code that carries a built shape is
  a different thing, and [code splicing](code-splicing.md) owns it.
- *How the kinds are ordered — decided.* A smaller syntax lies under a larger
  one wherever it can stand in its place: a literal or a symbol is an
  expression of one part, and an expression is a block of one statement, so a
  body slot typed `Block` takes `#(x)`. A block is no expression, since a slot
  wanting one statement cannot take several.
- *The binder-slot raw-part types — decided.* They stay and become code kinds:
  whatever a program can write, it can hold as a value. `NameToken` is the
  symbol, with the value and type names under it.

## Dependencies

The sigiled code channel is [code names](code-names.md)'s; code equality
through bindings is [eval-scope](eval-scope.md)'s; fragments, splicing and a
run-time block's shape are [code splicing](code-splicing.md)'s.

**Requires:** none.

**Unblocks:**

- [Dispatch](dispatch.md) — `ATTR`'s symbol label, `EVAL`'s operand, and code-typed parameters.
- [Quotes resolve where they are written](eval-scope.md) — the code representation a quote's bindings ride in.
- [Code names](code-names.md) — the code kinds a sigiled name holds.
- [Code splicing](code-splicing.md) — the code kinds a fragment and a splice are typed by.
- [Dict defaults](dict-defaults.md) — `_` parses as a dict key.
