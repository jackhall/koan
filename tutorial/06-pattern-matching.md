# Pattern matching

`MATCH` runs exactly one of several arms, chosen by the value you hand it. It
has two forms, and which one you write decides how the arms' guards are read.
`MATCH … UNDER <Union> WITH` reads every guard as a **variant of that union**
and hands the arm the variant's payload — this is the
[tagged-union](05-tagged-unions.md) form. Plain `MATCH … WITH` reads every guard
as a **type** or a **literal value**, branching on the value itself (a plain
`Number`, a `Str`, an untagged `:(Number | Str)`, the number `0`). Unlike
[variant dispatch](05-tagged-unions.md#dispatching-on-a-variant), a match over a
union gives you the payload to work with.

## The shape of a match

```koan
UNION Maybe = #{Some: Number, None: Null}
LET m = (Maybe.Some 42)
MATCH m UNDER Maybe WITH #{Some: (PRINT "got a value"), None: (PRINT "nothing")}
```

```text
got a value
```

A match has three parts: the value to inspect, `UNDER <Union>` naming the union
its guards come from, and a `WITH` dict of arms. The arms are written
`#{<guard>: (<body>), …}`, a dict of quotes: each body is quoted because only one
of them runs (see
[What is quoted and what is bare](10-quoting.md#what-is-quoted-and-what-is-bare)).

The arm whose guard admits the value runs, and its value is the whole `MATCH`'s.
Inside an arm, the name `it` is bound to what the guard admitted — under a
union, the variant's payload — so a match is also how you *unwrap* a union:

```koan
UNION Maybe = #{Some: Number, None: Null}
LET m = (Maybe.Some 42)
PRINT
  MATCH m UNDER Maybe WITH #{Some: (it), None: (0)}
```

```text
42
```

A `MATCH` declares no result type. Its type is worked out from its arms: here
both arms produce a `Number`, so the whole `MATCH` is a `Number` and slots
straight into `PRINT`. When the arms produce different types, the `MATCH` may
be any of them. To check a `MATCH`'s value yourself, ascribe it with `:!`:

```koan
LET twice = (FN :{x :Any} -> Any = #((MATCH x WITH #{Number: (x + x), Str: (it)}) :! Number))
PRINT (twice {x = 4})
PRINT (twice {x = "four"})
```

```text
8
error: Str does not satisfy its ascription Number
```

## Matching on a type or a value

Without `UNDER`, `MATCH` branches on the value itself. A guard is a type — a
type name such as `Number`, or a sigiled type such as `:(Number | Str)` — or a
literal: a number, a string, `true`, `false` or `null`:

```koan
MATCH 42 WITH #{Number: (PRINT "a number"), Str: (PRINT "a string")}
```

```text
a number
```

A type guard admits every value of its type. A literal guard admits a value
equal to it, as `==` compares them, so a [tagged](05-tagged-unions.md) value
never matches a plain literal: `(Meters 1)` is not the number `1`.

Inside an arm, `it` has the guard's type, just as a function's parameter has the
type it declares. Under `:(LIST OF Any)`, `it` is a list of anything, and a call
that dispatches on it picks the overload for that type, whatever the list holds:

```koan
EXPR #(KIND xs :(LIST OF Number)) -> Str = #("numbers")
EXPR #(KIND xs :(LIST OF Any)) -> Str = #("anything")
LET xs = [1 2 3]
PRINT (KIND xs)
PRINT (MATCH xs WITH #{:(LIST OF Any): (KIND it)})
```

```text
numbers
anything
```

## The most specific arm wins

The arms are a dict, so the order you write them in says nothing. When several
guards admit a value, the **most specific** one runs, as
[dispatch](03-names-and-dispatch.md) picks the most specific overload: a literal
beats every type, and a narrower type beats a wider one. A `_` key is the
default arm: it admits anything and loses to every other guard.

```koan
LET describe = (FN :{x :Any} -> Str = #(
  MATCH x WITH #{0: ("zero"), Number: ("some number"), Str: ("a string"), _: ("something else")}
))
PRINT (describe {x = 0})
PRINT (describe {x = 7})
PRINT (describe {x = "hi"})
PRINT (describe {x = true})
```

```text
zero
some number
a string
something else
```

A value no guard admits is an error, and so is a value admitted by two guards
with neither more specific than the other. Where the guards and the value's type
already show it, the error comes before the program runs:

```koan
MATCH 1 WITH #{:(Number | Str): (1), :(Number | Bool): (2)}
```

```text
error: <input>:1:1: ambiguous `MATCH`: 2 arms admit Number and none ranks first
```

Writing the same guard twice — a type under two spellings, one literal, one
label, or `_` — is refused too.

## Every variant must be covered

A match has no implicit fallthrough. A `MATCH … UNDER` must have an arm for
**every** variant of its union, and a missing one is an error at the match
itself — before any value is looked at:

```koan
UNION Maybe = #{Some: Number, None: Null}
LET m = (Maybe.None null)
MATCH m UNDER Maybe WITH #{Some: (PRINT "got")}
```

```text
error: <input>:3:1: this `MATCH … UNDER :(Some | None)` has no arm for None; add a `_` arm to default it
```

Cover every variant the union declares — or add a `_` key, which stands in for
all the ones you left out:

```koan
UNION Color = #{Red: Null, Green: Null, Blue: Null}
LET c = (Color.Blue null)
MATCH c UNDER Color WITH #{Red: (PRINT "red"), _: (PRINT "some other colour")}
```

```text
some other colour
```

A named arm always wins over `_`, and `_` binds `it` to the payload exactly as a
named arm does. What `_` does *not* do is widen the match: it defaults the
union's variants, so a value that belongs to no variant of the union is still an
error. The same check refuses a guard naming no variant of the union, and a
`UNDER` clause that is not a union at all.

## Writing arms across lines

For more than a couple of arms, open the `#{` at the end of the `MATCH` line and
write one arm per line, indented inside it. The commas between entries are
optional:

```koan
UNION Color = #{Red: Null, Green: Null, Blue: Null}
LET c = (Color.Green null)
MATCH c UNDER Color WITH #{
  Red: (PRINT "red")
  Green: (PRINT "green")
  Blue: (PRINT "blue")
}
```

```text
green
```

An arm body that needs several steps writes each step as its own group, and its
last expression is the arm's value:

```koan
Some: ((PRINT "found it") (it))
```

## Recursion: the iteration idiom

Matching on a union is how Koan expresses iteration. A recursive function peels
one layer per call and stops at the base variant. Model the count as a union
where each `Succ` wraps a smaller value and `Zero` is the base:

```koan
UNION Nat = #{Zero: Null, Succ: Nat}
EXPR #(COUNTDOWN n :Nat) -> Str = #(
  MATCH n UNDER Nat WITH #{
    Zero: (PRINT "liftoff")
    Succ: ((PRINT "tick") (COUNTDOWN it))
  }
)
LET zero = (Nat.Zero null)
LET one = (Nat.Succ zero)
LET two = (Nat.Succ one)
LET three = (Nat.Succ two)
COUNTDOWN three
```

```text
tick
tick
tick
liftoff
```

Each `Succ` arm prints `tick` and recurses on `it` — the wrapped predecessor —
until `COUNTDOWN` reaches `Zero`. The `MATCH` is the last thing `COUNTDOWN`
does, so the recursive call at the end of its arm is in tail position, and this
runs in constant stack space no matter how deep the count.

A loop written this way also keeps very little memory while it runs. What it
holds on to is what its tail call hands to the next round: the arguments of
that call, and whatever those values reach. Everything else a round builds —
its bindings, the values it only inspected, the results of the helpers it
called along the way — is dropped when the next round starts. A loop that only
walks a structure therefore runs in the same memory however long the structure
is, and a loop that builds an accumulator holds the accumulator and little
more:

```koan
UNION Nat = #{Zero: Null, Succ: Nat}
EXPR #(COUNTDOWN n :Nat) -> Str = #(
  MATCH n UNDER Nat WITH #{
    Zero: (PRINT "liftoff")
    Succ: ((PRINT "tick") (COUNTDOWN it))
  }
)
EXPR #(DOUBLE n :Nat acc :Nat) -> Nat = #(
  MATCH n UNDER Nat WITH #{
    Zero: (acc)
    Succ: (DOUBLE it (Nat.Succ (Nat.Succ acc)))
  }
)
LET zero = (Nat.Zero null)
LET two = (Nat.Succ (Nat.Succ zero))
COUNTDOWN (DOUBLE two zero)
```

```text
tick
tick
tick
tick
liftoff
```

Each round of `DOUBLE` hands on `it`, the part of the count still to walk, and
the accumulator, two `Succ` layers deeper than the one it was handed. Those are
what the loop keeps: the count it walks, which already existed, and the
accumulator, which grows with the answer it is building. Nothing else from
earlier rounds survives. A value the loop reads from outside it — a top-level
binding, a member of a module, a parameter of the function the loop is defined
in — is handed on from where it already rests, so passing it along every round
costs nothing however large it is.

Next: [Records](07-records.md).
