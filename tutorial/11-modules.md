# Modules

A module groups related bindings under a name. Modules are how Koan organizes
larger programs, and — paired with signatures and [functors](12-functors.md) —
how it supports generic, reusable components.

## Declaring and reading a module

`MODULE <name> = (<body>)` declares a module. A module is a *value*, so its name
is a plain lowercase identifier — `geometry`, not `Geometry`. The body is a
sequence of `LET` bindings; read a member back with the `.` operator:

```koan
MODULE geometry = (LET pi = 3.14159)
PRINT geometry.pi
```

```text
3.14159
```

A module body with more than one binding wraps its members in a single pair of
parentheses, one member per indented line. The outer parentheses keep the
members as separate statements — without them, the indented lines would flatten
into a single expression:

```koan
MODULE origin = (
  LET x = 0
  LET y = 0
)
PRINT origin.x
PRINT origin.y
```

```text
0
0
```

Modules nest, and `.` chains through them:

```koan
MODULE outer =
  MODULE inner =
    LET value = 7
PRINT outer.inner.value
```

```text
7
```

Reading a member that doesn't exist is an error:

```koan
MODULE geometry = (LET pi = 3.14159)
geometry.tau
```

```text
error: shape error: module `geometry` has no member `tau`
```

Naming a module with a capitalized token is an error: capitalized names are
reserved for *types*, and a module is a value.

```koan
MODULE Geometry = (LET pi = 3.14159)
```

```text
error: shape error: module `Geometry` is named with a Type token, but a module is a value — the Type-token namespace names what can type a field. Name it snake_case, e.g. `geometry`
```

## Signatures

A signature is the *type* of a module — it describes the members a module must
provide, without supplying them. `SIG <Name> = (<body>)` declares one, with
`VAL <name> :<Type>` for each required value member:

```koan
SIG HasLabel = (VAL label :Str)
```

A signature on its own produces no output; it's a description. You connect a
module to a signature by **ascribing** it.

## Ascription

Ascribing a module to a signature checks that the module provides the
signature's members and gives back a module viewed through that signature. There
are two forms, written with the `:!` and `:|` operators:

```koan
SIG HasLabel = (VAL label :Str)
MODULE widget = (LET label = "button")
LET named = (widget :! HasLabel)
PRINT named.label
```

```text
button
```

`:!` is **transparent** ascription and `:|` is **opaque**. Both check the
module against the signature and expose its value members. The difference is in
how they treat *abstract type members* a signature can declare: transparent
ascription leaves those types visible as their underlying definition, while
opaque ascription hides them behind the signature, so callers can only use them
through the operations the signature provides. For a module with only value
members, the two behave the same.

If the module is missing a required member, ascription fails:

```koan
SIG HasLabel = (VAL label :Str)
MODULE plain = (LET other = 1)
plain :! HasLabel
```

```text
error: shape error: module does not satisfy signature `SIG (label: Str)`: missing member `label`
```

The result of an ascription is itself a module, so it binds under a lowercase
name (`named` above) like every other module. A *signature* name, by contrast, is
capitalized (`HasLabel`) — a signature is a type.

## Opening a module with `USING`

`USING <module> SCOPE (<body>)` runs the body with the module's members brought
directly into scope, so you can name them without the `<module>.` prefix:

```koan
MODULE greetings = (LET hello = "hi there")
PRINT
  USING greetings SCOPE (hello)
```

```text
hi there
```

Functions defined in the module come into scope too, so their shapes dispatch
inside the block:

```koan
MODULE doubling =
  LET dbl =
    FN (DOUBLE x :Number) -> Number = (x)
PRINT
  USING doubling SCOPE (DOUBLE 21)
```

```text
21
```

Next: [Functors](12-functors.md).
