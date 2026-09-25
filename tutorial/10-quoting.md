# Quoting and evaluating

Normally an argument is evaluated before the expression around it runs. Two
prefix sigils let you override that: capture an expression as a value without
running it, and run such a captured expression later.

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

## Evaluating with `$`

`$(<expr>)` *evaluates*: it takes a value, and if that value is a quoted
expression, runs it in the current scope. Pairing the two, the captured action
runs only when evaluated:

```koan
LET action = #(PRINT "hi")
PRINT "about to run it"
$(action)
```

```text
about to run it
hi
```

Together, `#` and `$` let you move a piece of unevaluated code through positions
that would otherwise run it eagerly, and run it where you choose. Evaluating a
value that *isn't* a quoted expression is an error:

```koan
LET n = 5
$(n)
```

```text
error: type mismatch for argument 'expr': expected KExpression, got Number
```

## Passing code to a function you wrote

Every argument evaluates before the call it belongs to. That is the whole rule,
and it holds for the functions you define as much as for the built-in ones. So
a function that wants *code* rather than a result declares a parameter typed
by a [kind of code](#what-kind-of-code-a-quote-is) — `:Expression` takes one
statement — and is called with a quote:

```koan
EXPR #(TWICE body :Expression) -> Any = #(
  $(body)
  $(body)
)
TWICE #(PRINT "hi")
```

```text
hi
hi
```

`body` receives the quoted expression as a value; each `$(body)` runs it. Any
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
  $(body)
  $(body)
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
one typed `:Name` takes only a lone name. A parenthesis you write is kept:
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
