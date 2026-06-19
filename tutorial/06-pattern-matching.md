# Pattern matching

`MATCH` branches on the tag of a [tagged-union](05-tagged-unions.md) value (or a
boolean) and runs exactly one branch. Unlike [variant
dispatch](05-tagged-unions.md#dispatching-on-a-variant), a match gives you the
payload to work with.

## The shape of a match

```koan
UNION Maybe = (Some :Number None :Null)
LET m = (Maybe (Some 42))
MATCH (m) -> :Str WITH (Some -> (PRINT "got a value") None -> (PRINT "nothing"))
```

```text
got a value
```

A match has three parts: the value to inspect, a **result type** written
`-> :Type`, and a `WITH` list of branches. The `-> :Type` is required — it
declares the type the whole `MATCH` expression produces, and every branch body
must produce a value of that type (just like a function's return type). Each
branch is `<Tag> -> (<body>)`.

Only the branch whose tag matches runs. Inside it, the name `it` is bound to the
matched value's payload, so a match is also how you *unwrap* a union:

```koan
UNION Maybe = (Some :Number None :Null)
LET m = (Maybe (Some 42))
PRINT
  MATCH (m) -> :Number WITH
    Some -> (it),
    None -> (0)
```

```text
42
```

Here the result type is `:Number`, the `Some` branch returns the unwrapped
payload `it`, and because the whole `MATCH` is a `Number` it slots straight into
`PRINT`.

`MATCH` also works on a boolean, where the two "tags" are `true` and `false`:

```koan
MATCH true -> :Str WITH (true -> (PRINT "yes") false -> (PRINT "no"))
```

```text
yes
```

## Every case must be covered

A match has no implicit fallthrough. If the value's tag has no branch, it's an
error:

```koan
UNION Maybe = (Some :Number None :Null)
LET m = (Maybe (None null))
MATCH (m) -> :Str WITH (Some -> (PRINT "got"))
```

```text
error: shape error: inexhaustive match = no branch for `None`
```

Cover every variant the value could hold. (For catching *errors* with a
wildcard, see [`TRY`](09-errors.md), which is a different construct.)

## Writing branches across lines

The single-line form above keeps all branches inside one set of parentheses.
For more than a couple of branches, spread them across lines. The rule has two
parts, and you need both:

1. Indent the branches **deeper** than the `MATCH` line.
2. End every branch line **except the last** with a comma.

```koan
UNION Color = (Red :Null Green :Null Blue :Null)
LET c = (Color (Green null))
MATCH (c) -> :Str WITH
  Red -> (PRINT "red"),
  Green -> (PRINT "green"),
  Blue -> (PRINT "blue")
```

```text
green
```

The trailing commas are what chain the branches into a single list; without
them, deeper indentation alone won't group them and the match fails to find its
branches. (Equivalently, you can wrap the whole branch list in parentheses and
indent inside it.)

A branch body that needs several steps is wrapped in one group, and its last
expression is the branch's value:

```koan
Some -> ((PRINT "found it") (it))
```

## Recursion: the iteration idiom

Matching on a union is how Koan expresses iteration. A recursive function peels
one layer per call and stops at the base variant. Model the count as a union
where each `Succ` wraps a smaller value and `Zero` is the base:

```koan
UNION Nat = (Zero :Null Succ :Nat)
FN (COUNTDOWN n :Nat) -> Str =
  MATCH (n) -> :Str WITH
    Zero -> (PRINT "liftoff"),
    Succ -> ((PRINT "tick") (COUNTDOWN it))
LET zero = (Nat (Zero null))
LET one = (Nat (Succ zero))
LET two = (Nat (Succ one))
LET three = (Nat (Succ two))
COUNTDOWN three
```

```text
tick
tick
tick
liftoff
```

Each `Succ` branch prints `tick` and recurses on `it` — the wrapped predecessor
— until `COUNTDOWN` reaches `Zero`. The recursive call is in tail position, so
this runs in constant stack space no matter how deep the count.

Next: [Records](07-records.md).
