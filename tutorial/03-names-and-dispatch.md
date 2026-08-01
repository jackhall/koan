# Names, binding, and dispatch

Two ideas underpin everything else in Koan: how names come to refer to values,
and how Koan decides what an expression *does*. This chapter covers both.

## Binding names with `LET`

`LET <name> = <value>` introduces a name and binds it to a value. The
expression evaluates to the bound value, so one declaration can chain
directly inside another declaration's value slot:

```koan
LET answer = 42
LET doubled = (LET copy = answer)
PRINT doubled
PRINT copy
```

```text
42
42
```

Those chain slots — plus statement position (a line of a program, a module or
function body) — are the only places a declaration may appear. A declaration
in an eagerly evaluated value position (a call argument, a list or dict
element, an operator operand) is a structured error:

```koan
PRINT (LET doubled = 42)
```

```text
error: binder declaration in an eagerly evaluated sub-expression `LET doubled = 42`; a binder must be a statement, a body, or nested in another binder's declaration slot
  in PRINT <staged> (<bind>)
```

### Names are lexical

A name is visible only to expressions that come *after* its binding. Referring
to a name before it is bound is an error, even if a binding appears later:

```koan
LET y = x
LET x = 42
PRINT y
```

```text
error: unbound name 'x'
```

Put the binding first and the reference resolves:

```koan
LET x = 42
LET y = x
PRINT y
```

```text
42
```

Binding the same name twice in one scope is an error:

```koan
LET total = 1
LET total = 2
```

```text
error: name 'total' is already bound in this scope
```

A nested scope — a function body, for instance — may *shadow* an outer name
with a fresh binding of its own; that is allowed and is how local variables
work. Re-binding within the *same* scope is what's rejected.

Two kinds of binder bend the "must come first" rule, because they're resolved
differently:

- **Function bodies** are re-resolved every time the function is called, not
  when it's defined. So sibling functions can call each other regardless of the
  order they're written in — mutual recursion just works.
- **A type can refer to itself.** A union or record type may name itself in its
  own definition (a list whose tail is another list, say). Two *different* types
  that refer to each other need a [`RECURSIVE TYPES`](08-newtypes.md#mutually-recursive-types)
  block to be declared together.

## Token classes

Before dispatch can match anything, the parser sorts every non-literal word
into one of three classes by its casing. The class decides the role a word can
play:

| Class        | Rule                                                              | Examples                  |
|--------------|------------------------------------------------------------------|---------------------------|
| **Keyword**  | a pure-symbol token, or all letters with ≥2 uppercase and no lowercase | `=`, `->`, `LET`, `MATCH` |
| **Type**     | starts uppercase **and** has at least one lowercase letter        | `Number`, `Point`, `Maybe`|
| **Identifier**| starts lowercase or `_`                                          | `x`, `greeting`, `my_var` |

There is a deliberate gap. A word that starts with an uppercase letter but fits
neither the keyword shape (it has a lowercase letter, or only one capital) nor
the type shape (it has no lowercase letter) is a **parse error**, not a
fallback identifier. So a lone capital like `T`, or an all-caps-with-digits word
like `K9`, is rejected — Koan reserves uppercase-leading shapes for types and
keywords and won't guess which you meant. In practice:

- Pick **type names** with at least one lowercase letter: `Elem`, `Key`,
  `Value`, `Maybe` — never a single capital.
- Pick **keywords** (the fixed words in a function you define) with two or more
  capitals and no lowercase: `DOUBLE`, `SWAP`, `THEN`.

## Dispatch: matching by shape

When you write an expression, Koan decides what to run by matching the *shape*
of what you wrote against the shape of every function in scope. A shape is the
expression's fixed **keywords** together with its typed **slots** — the names,
literals, and sub-expressions that fill the gaps between the keywords.

- **Keywords must match exactly.** They are the fixed skeleton of a shape.
- **Slots match by type**, and the most specific matching type wins.

A keyword is not a name. You can't look one up, bind it, or pass it around, and
a keyword on its own means nothing — it's only meaningful as part of a shape.
Identifiers are the opposite: a bare identifier in a value position is a name
lookup. So in `PRINT answer`, `PRINT` is a keyword that selects a shape, and
`answer` is a slot that resolves to a value.

Because matching is by shape, two functions can share keywords as long as their
slots differ by type. Koan routes each call to the most specific match:

```koan
FN (DESCRIBE x :Number) -> Str = ("a number")
FN (DESCRIBE x :Str) -> Str = ("a string")
PRINT (DESCRIBE 7)
PRINT (DESCRIBE "hi")
```

```text
a number
a string
```

Both definitions share the `DESCRIBE` keyword; the argument's type picks the
branch. This is the same mechanism the built-in forms use — `PRINT` in a
single-slot context is simply the shape nothing else matches. Defining a
function, as the next chapter shows, is how you add new shapes of your own.

Next: [Functions](04-functions.md).
