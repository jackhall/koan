# Functors

A functor is a module parameterized by another module — a function from modules
to modules. Functors are how you write a component once and specialize it to
many implementations: anything that supplies the required signature can be
plugged in.

## Defining and applying

`FUNCTOR (<keyword> <param> :<Signature>) -> <ReturnType> = (<body>)` declares a
functor. The parameter is a module constrained by a signature, and the body
builds and returns a new module. You apply it by calling its keyword with a
module that satisfies the parameter's signature:

```koan
SIG Ordered = (VAL compare :Number)
MODULE IntOrder = (LET compare = 7)
LET IntOrderV = (IntOrder :! Ordered)
FUNCTOR (MAKESET elem :Ordered) -> Module =
  MODULE Built =
    LET sample = (elem.compare)
LET NumberSet = (MAKESET IntOrderV)
PRINT NumberSet.sample
```

```text
7
```

`MAKESET` takes any module satisfying `Ordered` and builds a module around it,
reading the argument's members with `.` just like any module. The argument must
be a module that has been ascribed to a signature; applying a functor to a raw,
unascribed module fails to match. Each application is *generative* — it produces
a fresh module distinct from every other application.

The return type must denote a module-like thing (a module, signature, or
functor); a functor that claims to return an ordinary value is rejected at
definition:

```koan
SIG Ordered = (VAL compare :Number)
FUNCTOR (BADMAKE x :Ordered) -> Number = (5)
```

```text
error: shape error: FUNCTOR return-type slot must denote a module, signature, or functor; got `Number`
```

## Specializing signatures with `WITH`

A signature can declare an *abstract* type member alongside its value members,
written `TYPE <TypeName>`, and have other members refer to it. `WITH` pins such
a type member to a concrete type, producing a more specific signature:

```koan
SIG Ordered = (
  TYPE Carrier
  VAL compare :Carrier
)
LET IntOrdered = (Ordered WITH {Carrier = Number})
MODULE Ints = (
  LET Carrier = Number
  LET compare = 5
)
LET View = (Ints :! IntOrdered)
PRINT View.compare
```

```text
5
```

`Ordered WITH {Carrier = Number}` is `Ordered` with its `Carrier` slot fixed to
`Number`. Pinning a slot that the signature doesn't declare is an error
(`<Sig> has no abstract type slot ...`). A related form, `TYPE (Type AS Wrap)`,
declares a *higher-kinded* type member — a slot that takes a type and produces a
type — for signatures that abstract over type constructors rather than plain
types.

---

That completes the tour of the language as it stands. For the shape of what's
not built yet — arithmetic and comparison operators, loops, comments,
user-declared traits — see the project's roadmap. Back to the
[README](README.md) for the full chapter list.
