# Quoting and evaluating

Normally an argument is evaluated before the expression around it runs. The
`#` sigil overrides that: it captures an expression as a value without running
it, and `EVAL` runs such a captured expression later.

## Quoting with `#`

`#(<expr>)` *quotes*: it captures the parenthesized expression as a value
without evaluating it. The captured expression is a value like any other — you
can bind it, pass it, and store it — and nothing inside it runs until you ask:

```koan
LET action = #(PRINT "hi")
PRINT "nothing ran yet"
```

```text
nothing ran yet
```

The `PRINT "hi"` never executed; `action` just holds it as data.

## Running code with `EVAL`

`EVAL <code>` runs a quote's code. Pairing the two, the captured action runs
only when evaluated:

```koan
LET action = #(PRINT "hi")
PRINT "about to run it"
EVAL action
```

```text
about to run it
hi
```

Together, `#` and `EVAL` let you move a piece of unevaluated code through
positions that would otherwise run it eagerly, and run it where you choose.
Evaluating a value that isn't code is an error.

## Where a quote's names bind

A quote carries its code somewhere else to run, so a name in it does not
silently pick up whatever that name means where the code ends up. A name
written plainly in a quote is a **hole**: it binds only to a builtin, to a
binder in the quote's own code, or to a name you supply with `USING` (below).
So this is an unbound-name error when `EVAL` runs it, although `x` is bound
right beside it:

```koan
LET x = 7
EVAL #(PRINT x)
```

To take a name from where the quote is written, mark it with `$`. `$x` binds
`x` when the quote is made, and the code keeps that binding wherever it goes:

```koan
LET x = 7
LET show = #(PRINT $x)
EVAL show
```

```text
7
```

`$` binds a name; it never runs anything. A computed value enters a quote by
binding it first — `LET v = (…)`, then `#(… $v …)`. A `$` on a word marks the
name the word starts with, so `$point.x` reads the field `x` of the `point`
where the quote is written. Glued to a parenthesized group, `$(…)` marks one
keyworded use, such as `$(GREET "bob")`, so that `GREET` resolves where the
quote is written rather than being a hole.

`code USING <record>` fills a quote's holes by name, from the record's fields
or a module's members, and leaves any hole the record does not name a hole. A
field that names no hole is ignored, so one record can serve several quotes:

```koan
LET greet = #(PRINT name)
EVAL (greet USING {name = "bob"})
```

```text
bob
```

The third mark, `\`, lets the code that runs a quote supply a name; see
[code parameters](#asking-for-names-a-quote-needs) below. A `$` or `\` means
something only inside a quote, and one written anywhere else is refused before
the program runs. Printing a quote shows it as written, marks included, never
the values its names are bound to.

## Passing code to a function you wrote

Every argument evaluates before the call it belongs to. That is the whole rule,
and it holds for the functions you define as much as for the built-in ones. So
a function that wants *code* rather than a result declares a parameter typed
by a [kind of code](#what-kind-of-code-a-quote-is) — `:Expression` takes one
statement — and is called with a quote:

```koan
EXPR #(TWICE body :Expression) -> Any = #(
  EVAL body
  EVAL body
)
TWICE #(PRINT "hi")
```

```text
hi
hi
```

`body` receives the quoted expression as a value; each `EVAL body` runs it. Any
expression that produces a quoted-expression value fills the slot just as well
— a name bound to a quote, or a call that returns one — because the slot takes
a value, not a spelling.

Forget the `#` and the argument is an ordinary group, so it runs before
`TWICE` is ever chosen. Writing

```koan
TWICE (PRINT "hi")
```

prints `hi` once — that is the argument evaluating — and *then* fails to
dispatch, because what reached the slot was the `Str` the print returned, not
code. Nothing is undone by the failure; the side effect had already happened.
The error names the missing quote:

```koan
EXPR #(TWICE body :Expression) -> Any = #(
  EVAL body
  EVAL body
)
LET greeting = "hi"
TWICE greeting
```

```text
error: dispatch failed for TWICE Str at <input>:6:1: no matching function: an argument evaluated before dispatch; write #(…) to pass the code itself
```

The diagnostic names each argument by the *type* dispatch matched it on, not by
its spelling — `greeting` had already evaluated to a `Str`, and a `Str` is what
failed to match an `:Expression` slot. The site after the expression is where to
read the spelling back.

Hence the rule for calling a form that takes code: **quote what must not run.**

`TWICE`'s own names never reach the code it receives. `TWICE #(PRINT x)` is an
unbound-name error whatever `TWICE`'s scope, parameters or body declare, and
`TWICE #(PRINT $x)` prints the caller's `x`.

## Asking for names a quote needs

A function that runs code may offer that code names of its own. The code asks
for one with `\`: `\it` binds where the code is run. The parameter says which
names it offers in its type, `:(Expression NEEDING #[it])`, and an `EVAL` of
that parameter supplies each of them as it stands where the `EVAL` is written:

```koan
EXPR #(WITH_FIVE body :(Expression NEEDING #[it])) -> Any = #(
  LET it = 5
  EVAL body
)
WITH_FIVE #(PRINT \it)
```

```text
5
```

A quote's type records the `\` names its own code does not bind, so
`#(PRINT \it)` is an `:(Expression NEEDING #[it])`, and a quote with none is
its plain kind. A parameter accepts a quote whose needed names its list covers,
and `:Code` accepts every quote. A name listed that is not bound where the
`EVAL` is written is refused before the program runs.

## What is quoted and what is bare

The built-in forms follow the same rule, so a reader can tell from any call
which of its parts run there:

- A part that runs later, conditionally or never is **quoted**: a function's
  body (`FN :{x :Number} -> Number = #(x)`), an `EXPR` head, and an operator's
  symbol (`OP #(+) OVER …`).
- A part that declares, or runs once where it is written, is **bare**: the name
  a `LET` binds, a type expression, and the body of a `MODULE`, a `GROUP`, a
  `USING … SCOPE`, a `TRY` or a `CATCH`.
- A part that names things as data is a **container of quotes**: the arms of a
  [`MATCH`](06-pattern-matching.md), the variants of a
  [`UNION`](05-tagged-unions.md), a `FOR ALL` group, the fields `FROM` projects
  and the members of a [`SIG`](11-modules.md).
- Every other part is an ordinary argument, evaluated before the form runs.

Writing a part the wrong way — a function body without its `#`, a `MATCH` whose
arms are not a `#{…}` — is refused before the program runs, with a message
saying how the part is written.

## Quoting each element of a list or dict

Glued to a list or dict literal, `#` quotes every element rather than the
literal as a whole. A parenthesized element is quoted as that group, and any
other element becomes a quote of itself:

```koan
LET names = #[x y]
LET arms = #{Some: (PRINT "got"), None: (PRINT "none")}
```

`names` is the list `[#(x) #(y)]`, and `arms` the dict
`{#(Some): #(PRINT "got"), #(None): #(PRINT "none")}`. A `_` key stays bare, and
in a `MATCH`'s arms it names the default arm. The result is an ordinary list or dict whose
elements happen to be code, which is exactly what the built-in forms above take
where they name things as data.

## What kind of code a quote is

A quote's type says what its body is, as written. The kinds form a tree, each
kind standing in for the one above it:

```text
Code
└─ Block               two or more statements
   └─ Expression       one statement
      ├─ Declaration   #(VAL x :Str), #(TYPE Carrier), a bodyless head
      │  └─ Binder     #(LET x = 1): a declaration that also binds where it is written
      ├─ Literal       #(42), #("y"), #(#(x))
      ├─ Symbol        one token
      │  ├─ Name       #(y), #(Carrier)
      │  └─ Keyword    #(+), #(NOOP)
      ├─ #(:(LIST OF Number))   a lone type expression
      └─ #(:{x :Number})        a lone record type
```

Every name in the tree but the last two is a type you can write in a slot, so
a parameter typed `:Block` takes any quote of code that could be a body, while
one typed `:Name` takes only a lone name. A kind [needing names](#asking-for-names-a-quote-needs),
such as `:(Block NEEDING #[it])`, lies under the same kind needing more. A parenthesis you write is kept:
`#(y)` is a `Name`, but `#((y))` is an `Expression` whose one part is the group
`(y)`, and `#((LET x = 1))` is an `Expression` rather than a `Binder`.

## The sigil must be glued

Each sigil and its opening bracket are a single unit — the `(`, or the `[` or
`{` after a `#`, must come immediately after the sigil, with no space:

```koan
LET action = # (1)
```

```text
error: parse error: expected '(', '[' or '{' after '#', found ' '
```

Next: [Modules](11-modules.md).
