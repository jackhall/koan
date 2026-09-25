# Functors

A functor is a module parameterized by another module — a function from modules
to modules. In koan a functor is not a separate construct: it is just a function
whose body builds and returns a module. Functors are how you write a component
once and specialize it to many implementations: anything that supplies the
required signature can be plugged in.

## Defining and applying

`EXPR (<keyword> <param> :<Signature>) -> <ReturnType> = (<body>)` — the ordinary
expression-shape binder from [chapter 4](04-functions.md). The parameter is a module
constrained by a signature, and the body builds and returns a new module. You
apply it by calling its keyword with a module that satisfies the parameter's
signature, exactly as you would call any other function:

```koan
SIG Ordered = (VAL compare :Number)
MODULE int_order = (LET compare = 7)
LET int_order_view = (int_order :! Ordered)
EXPR (MAKESET elem :Ordered) -> Module =
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

There is no return-slot restriction: a function may return anything, and a module is
just one of the things it can return. "Functor" names how you are *reading* the
function, not a kind the language tracks.

## The function is an ordinary value

Because a functor is an ordinary function, the combined
`LET <name> = FN EXPR …` statement from
[chapter 4](04-functions.md#two-kinds-of-function) binds it like any other function
— under a snake_case (value-class) name — and the value-side call form works
alongside the keyworded one:

```koan
SIG Ordered = (VAL compare :Number)
MODULE int_order = (LET compare = 7)
LET make_set = FN EXPR (MAKESET elem :Ordered) -> Module = (MODULE built = (LET sample = (elem.compare)))
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
LET MakeSet = FN EXPR (MAKESET elem :Ordered) -> Module = (MODULE built = (LET sample = 1))
```

```text
error: shape error: LET binder `MakeSet` is Type-classified but the bound value is a function (a value); rebind under a value-classified identifier instead (snake_case, e.g. `make_set`)
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
EXPR (MAKESET elem :Ordered) -> Module =
  MODULE built =
    LET compare = 3
LET number_set = (MAKESET int_order)
EXPR (ECHO elem :Ordered) -> :(TYPE OF elem) = (elem)
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
EXPR (ECHO elem :Ordered) -> elem = (elem)
```

```text
error: shape error: a return-type slot names a type, but `elem` is a value. For the type of a value — a module-valued parameter, say — write `-> :(TYPE OF elem)`
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
EXPR (OPEN b :(Number AS Boxed)) -> Str = ("a boxed number")
EXPR (OPEN b :(Str AS Boxed)) -> Str = ("a boxed string")
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

## Unions over type parameters: `UNION (Elem AS Option)`

A union can take type parameters too. List them before the `AS`, as for
`NEWTYPE`, and name them in the variants' payload types:

```koan
UNION (Elem AS Option) = (Some :Elem None :Null)
UNION (Elem AS Tree) = (Leaf :Null Node :{value :Elem, left :(Elem AS Tree), right :(Elem AS Tree)})
```

Each variant is a type constructor over *all* of the union's parameters, and a
payload may apply the union itself, as `Tree`'s `Node` does. Applying the union
applies every variant at once: `:(Number AS Option)` admits a `Some` holding a
number and a `None`, and `:(Result {Ok = Number, Error = Str})` either variant of
the built-in `Result`, which is declared this way:
`UNION (Ok Error AS Result) = (Ok :Ok Error :Error)`.

A value built through a variant infers each parameter from its payload, and a
parameter the payload says nothing about is `Never`. `Option.Some 1` has type
`:(Option.Some {Elem = Number})`, and `Option.None null` has type
`:(Option.None {Elem = Never})`. An applied constructor accepts anything built at
narrower arguments, so that `None` fits `:(Number AS Option)` and
`:(Str AS Option)` alike, and every one of these values fits the bare `Option`.
A payload that can't be read as the variant's type — a `Tree.Node` whose record
has no `left` field, or whose `value` is a number while its `left` holds strings
— is an error naming the variant and the payload's type.

Two things a parameterized union can't do: bound a parameter with `UNDER`, and
use a parameter inside a function type's parameter list — `(Take :(FN :{x :Elem}
-> Null))` is an error, while a function *returning* an `Elem` is fine.

## One definition at every type: `FOR ALL`

The `OPEN` overloads above name one boxed type each. When an operation works the
same way at *every* type, write it once and quantify over the type instead. A
`FOR ALL (<names>)` group sits between `EXPR` and the head, and the names it lists
can be used by the slots and the return:

```koan
NEWTYPE (Type AS Boxed)
EXPR FOR ALL (Elt) (BOX x :Elt) -> :(Elt AS Boxed) = (Boxed (x))
PRINT (BOX 7)
PRINT (BOX "hi")
```

```text
:(Boxed {Type = Number})(7)
:(Boxed {Type = Str})(hi)
```

One definition answered both calls. Each call works out what `Elt` stands for from
the type of the argument you passed — `BOX 7` reads `Elt = Number` and returns a
`:(Number AS Boxed)`, `BOX "hi"` reads `Elt = Str`. Nothing is written at the call
site to say so, and the body can use `Elt` as an ordinary type name. A name your
arguments never mention is refused when you define the function, because no call
could ever work it out.

A signature can declare a quantified member the same way, and a module satisfies it
with a single implementation:

```koan
SIG Boxes = (
  (TYPE (Type AS Wrap))
  (EXPR FOR ALL (Elt) (BOX _ :Elt) -> :(Elt AS Wrap))
)
MODULE boxing = (
  (NEWTYPE (Type AS Wrap))
  (EXPR FOR ALL (Elt) (BOX x :Elt) -> :(Elt AS Wrap) = (Wrap (x)))
)
LET boxes = (boxing :| Boxes)
PRINT (USING boxes SCOPE (BOX 7))
PRINT (USING boxes SCOPE (BOX "hi"))
```

```text
:(Wrap {Type = Number})(7)
:(Wrap {Type = Str})(hi)
```

The module ascribes **once**, not once per element type. A module offering only
`(EXPR (BOX x :Number) -> :(Number AS Wrap) = …)` is refused, and the error names the
quantifier the overload pinned down: one implementation has to hold at every `Elt`.

Note that `_` in the signature's head. A declaration has no body, so it has no use
for a parameter name — write `_` and give the slot its type. A definition names its
parameters because its body reads them, and two definitions that differ only in what
they call their parameters satisfy the same declaration.

A quantifier is not the same thing as a `:Type` parameter. `EXPR (MAKESET Elt :Type) …`
takes the type as an *argument*, written at the call (`MAKESET Number`).
`EXPR FOR ALL (Elt) …` takes no such argument: the type is worked out from what the
other arguments carry.

## Bounding a type parameter: `UNDER`

A bare `FOR ALL` name stands for any type at all — an ordinary value's, a type's,
or code's. To keep a parameter to one part of that, give it a **bound** with
`UNDER`:

```koan
EXPR FOR ALL (Elt UNDER Value) (KEEP x :Elt) -> Elt = (x)
LET pick = (FN FOR ALL ((Elt UNDER Number) Key) :{x :Elt y :Key} -> Elt = (x))
```

`KEEP` takes any ordinary value — a number, a string, a list, a module — but not a
quote: `#(1)` is code, and code does not lie under `Value`. `pick` works out `Elt`
from `x` as before, and a call whose `x` is not a number, such as
`pick {x = "a", y = 1}`, is refused just as a call that leaves `Elt` with no
answer is. `Key` is written bare, so it is bounded by `Any` and takes anything.

Each bounded name sits in parentheses of its own, beside the bare ones; a group
holding a single bounded name drops the outer pair, as `KEEP` does. A bound is one
type, so a union is written sigiled: `(Elt UNDER :(Number | Str))`. It may not name
another type parameter or a signature's abstract type, and it may not be `Never`,
which no value could ever satisfy.

A signature's type member takes a bound the same way:

```koan
SIG Counter = (
  (TYPE (Carrier UNDER Number))
  (VAL zero :Carrier)
)
MODULE ints = (
  (LET Carrier = Number)
  (LET zero = 0)
)
LET counter = (ints :| Counter)
```

A module satisfies `Counter` only when it binds `Carrier` to a type under
`Number`; one binding it to `Str` is refused. The opaque view still hides *which*
type `Carrier` is, but not its bound: `counter.zero` is admitted by a `:Number`
slot and compares equal to `0`. With a bare `TYPE Carrier`, the view would hide
even that `zero` is an ordinary value. A higher-kinded member such as
`TYPE (Type AS Wrap)` takes no bound, and neither does a `NEWTYPE` constructor.

## Both at once: `&`

`A | B` admits a value of *either* type; `A & B` admits only a value of *both*,
and a longer run chains as `|` does:

```koan
LET Text = :((Number | Str) & (Str | Bool))
LET Named = :(:{x :Number} & :{y :Str})
```

`Text` is `Str`, the one type the two unions share, and `Named` is the record type
`:{x :Number y :Str}`, which has both fields. Two types with nothing in common meet
at `Never`. Koan has no operator precedence, so `|` and `&` mix only through
parentheses: `Number | Str & Bool` is an error, and `:((Number | Str) & Bool)` says
which one you mean.

---

That completes the tour of the language as it stands. For the shape of what's
not built yet — arithmetic and comparison operators, loops, comments,
user-declared traits — see the project's roadmap. Back to the
[README](README.md) for the full chapter list.
