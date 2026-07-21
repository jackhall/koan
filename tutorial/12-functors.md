# Functors

A functor is a module parameterized by another module — a function from modules
to modules. In koan a functor is not a separate construct: it is just an `FN`
whose body builds and returns a module. Functors are how you write a component
once and specialize it to many implementations: anything that supplies the
required signature can be plugged in.

## Defining and applying

`FN (<keyword> <param> :<Signature>) -> <ReturnType> = (<body>)` — the ordinary
function binder from [chapter 4](04-functions.md). The parameter is a module
constrained by a signature, and the body builds and returns a new module. You
apply it by calling its keyword with a module that satisfies the parameter's
signature, exactly as you would call any other function:

```koan
SIG Ordered = (VAL compare :Number)
MODULE int_order = (LET compare = 7)
LET int_order_view = (int_order :! Ordered)
FN (MAKESET elem :Ordered) -> Module =
  MODULE built =
    LET sample = (elem.compare)
LET number_set = (MAKESET int_order_view)
PRINT number_set.sample
```

```text
7
```

`MAKESET` takes any module satisfying `Ordered` and builds a module around it,
reading the argument's members with `.` just like any module. A signature slot is
**structural**: any module whose own members satisfy `Ordered` is admitted, so
`(MAKESET int_order)` on the raw module works too — ascription (`:!` / `:|`) is a
way to *narrow* what the argument exposes, never a prerequisite for passing it.
Each application is *generative* — it produces a fresh module distinct from every
other application.

There is no return-slot restriction: an `FN` may return anything, and a module is
just one of the things it can return. "Functor" names how you are *reading* the
function, not a kind the language tracks.

## The function is an ordinary value

Because a functor is an ordinary function, `LET` binds it like any other function
value — under a snake_case (value-class) name — and the value-side call form works
alongside the keyworded one:

```koan
SIG Ordered = (VAL compare :Number)
MODULE int_order = (LET compare = 7)
LET make_set = (FN (MAKESET elem :Ordered) -> Module = (MODULE built = (LET sample = (elem.compare))))
LET a = (MAKESET int_order)
LET b = (make_set {elem = int_order})
PRINT a.sample
PRINT b.sample
```

```text
7
7
```

`(MAKESET int_order)` is the keyworded call; `(make_set {elem = int_order})` fills
the parameters by name through the bound function value. Binding it under a
Type-class (capitalized) name is an error — a function is a value, not a type:

```koan
SIG Ordered = (VAL compare :Number)
LET MakeSet = (FN (MAKESET elem :Ordered) -> Module = (MODULE built = (LET sample = 1)))
```

```text
error: type-class binding `MakeSet` expects a type value, got `:(FN (elem :SIG (compare: Number)) -> Module)`
```

## Modules in type position: `TYPE OF`

A module is a value, so a module name never names a type on its own — `x :int_order`
is not even valid syntax. To reach a module's *type*, ask for it: `TYPE OF <value>`
yields the type a value reports for itself, and a module reports its **signature** —
the interface its members add up to.

Write it in a slot to admit any module with that interface, or in a return type to
say "returns a module with this argument's interface", resolved per call:

```koan
SIG Ordered = (VAL compare :Number)
MODULE int_order = (LET compare = 7)
FN (MAKESET elem :Ordered) -> Module =
  MODULE built =
    LET compare = 3
LET number_set = (MAKESET int_order)
FN (ECHO elem :Ordered) -> :(TYPE OF elem) = (elem)
LET same = (ECHO number_set)
PRINT same.compare
PRINT (ECHO int_order)
```

```text
3
int_order
```

`ECHO` returns whichever module it was handed, and the returned module stays live
after the call — `same.compare` reads `3` out of the module `MAKESET` built. The
slot is **structural**: `m :(TYPE OF int_order)` admits any module whose members
satisfy `int_order`'s, the same test a signature slot runs. A dotted head projects
a single member instead of naming the whole interface: `-> elem.Carrier` as a return
type resolves to the argument module's `Carrier` type member.

`TYPE OF` is not module-specific — it reads any value's type, so `TYPE OF 5` is
`Number`. Naming a value directly where a type belongs is an error, and the message
points at the spelling above:

```koan
SIG Ordered = (VAL compare :Number)
FN (ECHO elem :Ordered) -> elem = (elem)
```

```text
error: shape error: FN return-type slot names a type, but `elem` is a value. For the type of a value — a module-valued parameter, say — write `-> :(TYPE OF elem)`
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
MODULE ints = (
  LET Carrier = Number
  LET compare = 5
)
LET view = (ints :! IntOrdered)
PRINT view.compare
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

## Declaring a type constructor: `NEWTYPE (Type AS Wrap)`

`TYPE (Type AS Wrap)` above only *declares a slot* inside a signature. To make a
real constructor a module can supply — or that you can wrap values with — use the
`NEWTYPE` form: `NEWTYPE (Type AS Wrapper)` declares a **type constructor** named
`Wrapper`. It reads like the application form `:(Number AS Wrapper)` with the
concrete type replaced by the placeholder `Type`.

Once declared, `Wrapper` wraps a value of any type, and the result carries the
*applied* type `:(<value's type> AS Wrapper)` — so you can dispatch on what's inside
the box:

```koan
NEWTYPE (Type AS Boxed)
FN (OPEN b :(Number AS Boxed)) -> Str = ("a boxed number")
FN (OPEN b :(Str AS Boxed)) -> Str = ("a boxed string")
PRINT (OPEN (Boxed (7)))
PRINT (OPEN (Boxed ("hi")))
```

```text
a boxed number
a boxed string
```

`Boxed (7)` builds a value whose type is `:(Number AS Boxed)`, and `Boxed ("hi")`
one of type `:(Str AS Boxed)`, so the two `OPEN` overloads dispatch on the boxed
type exactly as ordinary overloads dispatch on a plain argument type. Because the
declaration is valid inside a `MODULE` body, a module can declare `Wrapper` as the
concrete witness for a signature's `TYPE (Type AS Wrap)` slot — the missing piece
that lets a module satisfy a higher-kinded signature. The parameter names have to
match: a module supplying `NEWTYPE (Item AS Wrap)` does *not* satisfy a
`TYPE (Type AS Wrap)` slot, because the slot names its parameter `Type`.

## More than one parameter: `:(Ctor {Name = Type, …})`

A constructor can take several parameters — list them all before the `AS`. Applying
one binds each parameter by name, in a brace literal:

```koan
NEWTYPE (Key Val AS Pair)
LET NumToStr = :(Pair {Key = Number, Val = Str})
PRINT NumToStr
```

```text
:(Pair {Key = Number, Val = Str})
```

The names are what matter, not the order — writing `{Val = Str, Key = Number}` gives
the same type. Supplying a key the constructor doesn't declare, or leaving one out,
is an error that names the offending keys. The built-in `Result` is applied the same
way: `:(Result {Ok = Number, Error = Str})`.

`:(Number AS Boxed)` is shorthand for the one-parameter case — it fills the
constructor's only parameter, so it means exactly `:(Boxed {Type = Number})`. A
constructor with two or more parameters has to use the brace form, and it can only be
used in *type* position: `Boxed (7)` wraps one value and infers one type argument, so
there is nothing for a second parameter to be inferred from.

---

That completes the tour of the language as it stands. For the shape of what's
not built yet — arithmetic and comparison operators, loops, comments,
user-declared traits — see the project's roadmap. Back to the
[README](README.md) for the full chapter list.
