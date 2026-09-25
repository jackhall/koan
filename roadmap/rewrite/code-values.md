# Code as values

Quoted code typed by what it is — a literal, a symbol, a declaration, a binder,
an expression or a block — read by the shape builder through those types, and
written as a quote, or a container of quotes, wherever a builtin takes code.

**Problem.** Every quote and every raw group is typed `KExpression`, and a raw
identifier `Identifier` ([values/admission.rs](../../src/values/admission.rs)),
so no type tells a symbol from an expression, a block or a binding statement.
The raw-part types `Identifier`, `NameToken`, `TypeNameToken`, `KExpression`,
`SigiledTypeExpr` and `RecordType` are unordered, each only under `Code`. They
are also how a
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
parts run. An arm set, a `UNION`'s variants, a `FOR ALL` group and `FROM`'s
field list are each a bare run whose type says nothing about its parts, so the
[shape builder](../../src/scope/README.md) reads each by a rule of its own. A
`SIG` body is a bare run whose type says nothing about its members: a statement
that declares no member is refused only where the signature is elaborated, as
unsupported, and no type tells a `VAL`, a `TYPE` declarator or a bodyless head
from any other statement.
`Role::Label` marks three unlike slots: `ATTR`'s label, `VAL`'s member name and
`FROM`'s field list. `ATTR`'s label is typed `NameToken | Str`, so a field named
at run time is named by a string. And the parser peels every redundant paren,
so `#((y))` and `#(y)` are the same quote.

**Acceptance criteria.**

- The code family is a tree under `Code`: `Block` lies under `Code`;
  `Expression` under `Block`; `Declaration`, `Literal`, `Symbol`, a quoted
  `:(…)` (`SigiledTypeExpr`) and a quoted `:{…}` (`RecordType`) under
  `Expression`; `Binder` under `Declaration`; `Name` and `Keyword` under
  `Symbol`; a value name (`Identifier`) and a type name (`TypeNameToken`) under
  `Name`. The type spelled `KExpression` is spelled `Expression`, and
  `NameToken` is spelled `Symbol`. `Symbol`, `Literal`, `Block`, `Declaration`,
  `Binder`, `Name` and `Keyword` are spellable.
- A quote is typed by its body as written: `#(y)` is a name and `#(+)` a
  keyword, `#(42)`, `#("y")` and `#(#(x))` are literals, `#((y))` and
  `#(f x)` are expressions, `#(LET x = 1)` is a binder, `#(VAL x :Str)`,
  `#(TYPE Carrier)` and a quoted bodyless head are declarations that are no
  binder, and a quote of two or more statements is a block. A parenthesis the
  program writes is never peeled.
- `#[…]` and `#{…}` quote each element of the literal they are glued to: a
  paren-group element is quoted as that group, any other element as a one-part
  quote, and a `_` key or a record field name stays bare. The result is a
  container value of code, typed as any container is.
- Every builtin slot is read one of four ways, and the reading is its role's:
  as a written quote (a callable's body, an `EXPR` head, an `OP` symbol), as
  bare syntax that declares or runs where it is written (a binder name, a
  `VAL` name, a type expression, a `MODULE`, `GROUP`, `USING … SCOPE`, `TRY`
  or `CATCH` body), as a container of quotes (an arm set, a `UNION`'s
  variants, a `FOR ALL` group, a `SIG` body), or evaluated. The slot's type
  says what syntax fills it, a bare group counting as code of its own kind, and
  the shape builder refuses a part that does not admit its slot's type or is
  not written as its reading says, where the shape is built. No slot keeps a
  part raw.
- The shape builder builds each quote where it is written: a body as that
  callable's body shape, a head as its parameters, each arm as a block.
- Arms are written `#{<guard>: (<body>), …}` and typed `Dict(Name, Block)`,
  each entry a guard and a block binding `it`, and a `_` key is the default
  arm. The builder records each arm's guard as the type of `it` and that its
  block's last statement is in tail position exactly where the `MATCH` or
  `TRY` is.
- A union's variants are written `UNION <Name> = #{<Tag>: <Type>, …}` and typed
  `Dict(Name, TypeCode)`, where `TypeCode` is
  `TypeNameToken | SigiledTypeExpr | RecordType`; a parameterized union keeps
  its bare declarator.
- A `FOR ALL` group is written `FOR ALL #[<Name> …]` and typed `List(Name)`.
- A `SIG` body is written `SIG <Name> = #[(<member>) …]` and typed
  `List(Declaration)`, each element a `VAL`, a `TYPE` declarator, a `LET` of a
  type name, or a bodyless `EXPR`, `OP`, `UNARY OP` or `GROUP` head; a bodyless
  `GROUP`'s heads are written `GROUP FOLD LEFT = #[(<head>) …]` and typed the
  same. An element that is no declaration is refused where the shape is built.
- `FROM`'s field list is written `#[<name> …] FROM <record>`, typed
  `List(Name)`, and evaluated as any argument is.
- A binder name is written as a bare name, or a bare declarator group for
  `TYPE`, `UNION` and `NEWTYPE`; a quote in a name slot is refused where the
  shape is built. `VAL`'s name is a `Name` slot, and no `Label` role exists.
- The tutorial's snippets write quoted bodies and heads, and arms, variants,
  quantifiers, field lists and signature bodies as containers of quotes.
- `ATTR`'s label is a name: a bare name is the label itself and any other
  part is evaluated, so `ATTR p y` names the field `y` and `ATTR p (which)`
  reads `which`. No overload takes a `Str` label, and tutorial 07's field read
  named at run time is spelled `LET which = #(y)`.
- `EVAL`'s operand is typed `Code`.

**Directions.**

- *Metaprogramming model — decided.* Hygienic fexprs: a callee takes code
  through a slot typed by a code kind, the caller quotes it, and the quote's
  names resolve where it is written
  ([quotes resolve where they are written](eval-scope.md)). Koan has no
  expansion system and no global execution phases. Rewriting stays the shape
  builder's own, as the pairwise rewrite is, since a user's rewrite rule acts at
  a distance; and code generated per argument type, as Julia's `@generated`
  does, solves too narrow a problem.
- *What is written how — decided.* A part that runs later, conditionally or
  never is quoted; a part that declares, or runs where it is written, once, is
  bare; a part that names things as data is a container of quotes; every other
  part is evaluated. So a reader tells from a builtin's call which of its parts
  run, and a binder name stays bare.
- *The type says the syntax, the role says the reading — decided.* A bare
  group is a code part like a quote, kinded by its syntax, so an in-place body
  is typed `Block` and `FN … = #(x)` differs from `MODULE m = (…)` by its
  role's reading alone. The builder's check for a
  read-as-written part is one static admission against the slot's type plus
  the reading's form; no position list beside the table.
- *Containers name things as data — decided.* Where a slot's parts are things
  to name rather than code to run — an arm's guards, a union's tags, a
  quantifier's names, a projection's fields, a signature's members — the slot
  is a list or dict of
  code kinds, since a container's type states what each element is and an
  expression's cannot. Arm selection is by specificity, as dispatch selects a
  candidate ([control expression shapes and errors](control-and-errors.md)),
  so a dict's reading is honest.
- *`#[…]` and `#{…}` quote each element — decided.* The sugar for a container
  of quotes: `#{Some: (…)}` is `{#(Some): #(…)}`. A quoted container literal,
  `#([1 2])`, is an `Expression`.
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
  one wherever it can stand in its place: a literal, a symbol or a declaration
  is an expression of one statement, and an expression is a block of one statement,
  so a body slot typed `Block` takes `#(x)` and a head slot typed `Expression`
  takes a nullary head `#(NOOP)`. A lone scalar or name is an expression
  because dispatch evaluates it as a statement, in its own shape. A block is no
  expression, since a slot wanting one statement cannot take several.
- *Declarations are a kind — decided.* A quote whose statement declares a name
  or a shape is a `Declaration`: a `VAL`, a `TYPE` declarator, a bodyless head,
  or any binder. A `Binder` is the declaration that also installs where it is
  written, so `#(LET x = 1)` is a `Binder` and `#(VAL x :Str)` a `Declaration`
  only. The kind is read off the statement's builtin shape, not off a binder
  plan: a `TYPE` declarator carries a plan so the member scan can key on it,
  yet installs nothing. What the builder must see statically is a type. A
  written paren hides it: `#((LET x = 1))` is an `Expression`.
- *The binder-slot raw-part types — decided.* They stay and become code kinds:
  whatever a program can write, it can hold as a value. `NameToken` is the
  symbol, with `Name` under it holding the value and type names, and `Keyword`
  beside it.
- *Bounded quantifiers — decided.* A bounded `FOR ALL` group is a dict,
  `FOR ALL #{Elt: Number, Other: Any}`, typed `Dict(Name, TypeCode)`, so the
  type states each element; `UNDER` is kept only by
  `TYPE (<Name> UNDER <Type>)`.
- *A `SIG` body is a list of declarations — decided.* A signature's members
  are things to name, as a union's variants are, so the body is a list of
  member quotes, and so are the heads a bodyless `GROUP` declares. `LET x = 1`
  admits the type and is refused where the signature is elaborated, since no
  manifest value member exists; a type that says so would split `Binder` by
  channel, which waits on [code names](code-names.md), whose sigiled binder
  gives the split its third member. A type member stays a `TYPE` declarator in
  the list; a signature quantified at its head instead is
  [modules](modules.md)'s open direction.

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
