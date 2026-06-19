# Modules

A module groups related bindings under a name. Modules are how Koan organizes
larger programs, and — paired with signatures and [functors](12-functors.md) —
how it supports generic, reusable components.

## Declaring and reading a module

`MODULE <Name> = (<body>)` declares a module. The body is a sequence of `LET`
bindings; read a member back with the `.` operator:

```koan
MODULE Geometry = (LET pi = 3.14159)
PRINT Geometry.pi
```

```text
3.14159
```

A module body with more than one binding wraps its members in a single pair of
parentheses, one member per indented line. The outer parentheses keep the
members as separate statements — without them, the indented lines would flatten
into a single expression:

```koan
MODULE Origin = (
  LET x = 0
  LET y = 0
)
PRINT Origin.x
PRINT Origin.y
```

```text
0
0
```

Modules nest, and `.` chains through them:

```koan
MODULE Outer =
  MODULE Inner =
    LET value = 7
PRINT Outer.Inner.value
```

```text
7
```

Reading a member that doesn't exist is an error:

```koan
MODULE Geometry = (LET pi = 3.14159)
Geometry.tau
```

```text
error: shape error: module `Geometry` has no member `tau`
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
MODULE Widget = (LET label = "button")
LET Named = (Widget :! HasLabel)
PRINT Named.label
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
MODULE Plain = (LET other = 1)
Plain :! HasLabel
```

```text
error: shape error: module does not satisfy signature `HasLabel`: missing member `label`
```

The result of an ascription is itself a module, bound to a type name (`Named`
above) — capitalized with a lowercase letter, like every type.

## Opening a module with `USING`

`USING <Module> SCOPE (<body>)` runs the body with the module's members brought
directly into scope, so you can name them without the `Module.` prefix:

```koan
MODULE Greetings = (LET hello = "hi there")
PRINT
  USING Greetings SCOPE (hello)
```

```text
hi there
```

Functions defined in the module come into scope too, so their shapes dispatch
inside the block:

```koan
MODULE Doubling =
  LET dbl =
    FN (DOUBLE x :Number) -> Number = (x)
PRINT
  USING Doubling SCOPE (DOUBLE 21)
```

```text
21
```

Next: [Functors](12-functors.md).
