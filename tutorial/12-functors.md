# Functors

A functor is a module parameterized by another module — a function from modules
to modules. In koan a functor is not a separate construct: it is just a function
whose body builds and returns a module. Functors are how you write a component
once and specialize it to many implementations: anything that supplies the
required signature can be plugged in.

## Defining and applying

`EXPR #(<keyword> <param> :<Signature>) -> <ReturnType> = #(<body>)` — the ordinary
expression-shape binder from [chapter 4](04-functions.md). The parameter is a module
constrained by a signature, and the body builds and returns a new module. You
apply it by calling its keyword with a module that satisfies the parameter's
signature, exactly as you would call any other function:

```koan
SIG Ordered = #[(VAL compare :Number)]
MODULE int_order = (LET compare = 7)
LET int_order_view = (int_order :! Ordered)
EXPR #(MAKESET elem :Ordered) -> Module = #(
  MODULE built =
    LET sample = (elem.compare)
)
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
SIG Ordered = #[(VAL compare :Number)]
MODULE int_order = (LET compare = 7)
LET make_set = FN EXPR #(MAKESET elem :Ordered) -> Module = #(MODULE built = (LET sample = (elem.compare)))
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
SIG Ordered = #[(VAL compare :Number)]
LET MakeSet = FN EXPR #(MAKESET elem :Ordered) -> Module = #(MODULE built = (LET sample = 1))
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
SIG Ordered = #[(VAL compare :Number)]
MODULE int_order = (LET compare = 7)
EXPR #(MAKESET elem :Ordered) -> Module = #(
  MODULE built =
    LET compare = 3
)
LET number_set = (MAKESET int_order)
EXPR #(ECHO elem :Ordered) -> :(TYPE OF elem) = #(elem)
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
SIG Ordered = #[(VAL compare :Number)]
EXPR #(ECHO elem :Ordered) -> elem = #(elem)
```

```text
error: shape error: a return-type slot names a type, but `elem` is a value. For the type of a value — a module-valued parameter, say — write `-> :(TYPE OF elem)`
```

## Signatures over a type: `SIG … FOR ALL` and `WITH`

A signature can leave a type for each module to choose. Write a `FOR ALL` group
after the signature's name, and its members can name each type the group lists —
a **head parameter**:

```koan
SIG Ordered FOR ALL #[Carrier] = #[(VAL compare :Carrier)]
```

A module fits `Ordered` when one type for `Carrier` makes every member fit, and
koan works that type out from what the module defines, as a call works out a
`FOR ALL` name from its arguments: a module whose `compare` is a number fits
`Ordered` with `Carrier` standing for `Number`. A head parameter is one type per
module. A type a member needs anew at each use is written on the member instead,
as [below](#one-definition-at-every-type-for-all) shows.

`WITH` pins a head parameter to a type, producing an **application** of the
signature:

```koan
SIG Ordered FOR ALL #[Carrier] = #[(VAL compare :Carrier)]
LET IntOrdered = :(Ordered WITH {Carrier = Number})
MODULE ints = (LET compare = 5)
LET view = (ints :! IntOrdered)
PRINT view.compare
```

```text
5
```

An application may pin some of a signature's parameters and leave the rest to
the module. A module fitting `Ordered WITH {Carrier = Number}` fits `Ordered`
too, but two different pins of one parameter are unrelated types, even `Number`
and `Number | Str`. Pinning a name that is not one of the signature's head
parameters is an error.

Two applications meet with `&`, and a module fits the meet when it fits both.
A module defining `PUSH` once at `Number` and again at `Str` fits each pinned
`Stack`, their meet, and the bare `Stack`:

```koan
SIG Stack FOR ALL #[Elt] = #[(EXPR #(PUSH _ :Elt) -> :(LIST OF Elt))]
MODULE two = (
  (EXPR #(PUSH x :Number) -> :(LIST OF Number) = #([x]))
  (EXPR #(PUSH x :Str) -> :(LIST OF Str) = #([x]))
)
EXPR #(BOTH m :((Stack WITH {Elt = Number}) & (Stack WITH {Elt = Str}))) -> Str = #("both")
PRINT (BOTH two)
```

```text
both
```

A module defining `PUSH` only at `Number` fits `Stack` and
`Stack WITH {Elt = Number}`, but not `Stack WITH {Elt = Str}`.

## Declaring a type constructor: `NEWTYPE (Type AS Wrap)`

A type that takes a type is a **type constructor**: `NEWTYPE (Type AS Wrapper)`
declares one named `Wrapper`. It reads like the application `:(Number AS Wrapper)` with the
concrete type replaced by the placeholder `Type`.

Once declared, `Wrapper` wraps a value of any type, and the result carries the
*applied* type `:(<value's type> AS Wrapper)` — so you can dispatch on what's inside
the box:

```koan
NEWTYPE (Type AS Boxed)
EXPR #(OPEN b :(Number AS Boxed)) -> Str = #("a boxed number")
EXPR #(OPEN b :(Str AS Boxed)) -> Str = #("a boxed string")
PRINT (OPEN (Boxed (7)))
PRINT (OPEN (Boxed ("hi")))
```

```text
a boxed number
a boxed string
```

`Boxed (7)` builds a value whose type is `:(Number AS Boxed)`, and `Boxed ("hi")`
one of type `:(Str AS Boxed)`, so the two `OPEN` overloads dispatch on the boxed
type exactly as ordinary overloads dispatch on a plain argument type.

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
UNION (Elem AS Option) = #{Some: Elem, None: Null}
UNION (Elem AS Tree) = #{Leaf: Null, Node: :{value :Elem, left :(Elem AS Tree), right :(Elem AS Tree)}}
```

Each variant is a type constructor over *all* of the union's parameters, and a
payload may apply the union itself, as `Tree`'s `Node` does. Applying the union
applies every variant at once: `:(Number AS Option)` admits a `Some` holding a
number and a `None`, and `:(Result {Ok = Number, Error = Str})` either variant of
the built-in `Result`, which is declared this way:
`UNION (Ok Error AS Result) = #{Ok: Ok, Error: Error}`.

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
use a parameter inside a function type's parameter list — a variant
`Take: :(FN :{x :Elem} -> Null)` is an error, while a function *returning* an
`Elem` is fine.

## One definition at every type: `FOR ALL`

The `OPEN` overloads above name one boxed type each. When an operation works the
same way at *every* type, write it once and quantify over the type instead. A
`FOR ALL #[<names>]` group — a list of quoted names — sits between `EXPR` and the
head, and the names it lists can be used by the slots and the return:

```koan
NEWTYPE (Type AS Boxed)
EXPR FOR ALL #[Elt] #(BOX x :Elt) -> :(Elt AS Boxed) = #(Boxed (x))
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
arguments never mention has nothing to be worked out from, so it stands for its
[bound](#bounding-a-type-parameter) — `Any`, unless you give it another.

Two arguments at one type parameter need not share a type: `Elt` becomes the
smallest type holding both. A head's slots are read in **priority classes**,
though — one class per slot, in written order, unless a declaration such as
`EXPR #(BOTH 1 AND 1)` ranks them — and the first class that mentions `Elt`
fixes it for the later ones. So a head in written order asks for one type
across its slots, and ranking the slots alike lets them differ:

```koan
EXPR #(BOTH 1 AND 1)
EXPR FOR ALL #[Elt] #(BOTH x :Elt AND y :Elt) -> Str = #(PRINT Elt)
BOTH 1 AND "x"
BOTH 1 AND 2
```

```text
:(Number | Str)
Number
```

Without the first line, `BOTH 1 AND "x"` is refused: `1` fixes `Elt` to
`Number`, and `"x"` does not lie under it.

A quantified `FN` works the same way, called by name:

```koan
LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))
PRINT (pick {x = 1})
PRINT (pick {x = "s"})
```

```text
1
s
```

A quantified function is **called, never passed**: its name may stand only at
the head of a call, so `LET keep = [pick]` is an error, and so is a
`(FN FOR ALL …)` written anywhere but bound to a name or called on the spot. A
body hands back its last statement's value, so a body cannot end by binding one
either. To pass one, wrap it in an ordinary `FN` that calls it —
`(FN :{x :Number} -> Number = #(pick {x = x}))` goes anywhere a function does.
For the same reason a quantified type such as `:(FN FOR ALL #[Elt] :{x :Elt} -> Elt)`
is written only as a signature member's type, below; a slot that wants a
function usable at several types takes a module instead.

A signature can declare a quantified member the same way, and a module satisfies it
with a single implementation:

```koan
SIG Boxes = #[(EXPR FOR ALL #[Elt] #(BOX _ :Elt) -> :(LIST OF Elt))]
MODULE boxing = (EXPR FOR ALL #[Elt] #(BOX x :Elt) -> :(LIST OF Elt) = #([x]))
LET boxes = (boxing :| Boxes)
PRINT (USING boxes SCOPE (BOX 7))
PRINT (USING boxes SCOPE (BOX "hi"))
```

```text
[7]
[hi]
```

The module ascribes **once**, not once per element type, and each use inside the
block works out `Elt` afresh. A module offering only
`(EXPR #(BOX x :Number) -> :(LIST OF Number) = #([x]))` does not fit `Boxes`: one
implementation has to hold at every `Elt`. A `VAL` member may be quantified too,
`(VAL identity :(FN FOR ALL #[Item] :{x :Item} -> Item))`, and a block opening a
module through that signature calls `identity` by name.

Note that `_` in the signature's head. A declaration has no body, so it has no use
for a parameter name — write `_` and give the slot its type. A definition names its
parameters because its body reads them, and two definitions that differ only in what
they call their parameters satisfy the same declaration.

A quantifier is not the same thing as a `:Type` parameter. `EXPR #(MAKESET Elt :Type) …`
takes the type as an *argument*, written at the call (`MAKESET Number`).
`EXPR FOR ALL #[Elt] …` takes no such argument: the type is worked out from what the
other arguments carry.

## Bounding a type parameter

A `FOR ALL` name in a list stands for any type at all — an ordinary value's, a
type's, or code's. To keep a parameter to one part of that, give it a **bound**:
write the group as a dict of quotes, each name to the type it lies under:

```koan
EXPR FOR ALL #{Elt: Value} #(KEEP x :Elt) -> Elt = #(x)
LET pick = (FN FOR ALL #{Elt: Number, Key: Any} :{x :Elt y :Key} -> Elt = #(x))
```

`KEEP` takes any ordinary value — a number, a string, a list, a module — but not a
quote: `#(1)` is code, and code does not lie under `Value`. `pick` works out `Elt`
from `x` as before, and a call whose `x` is not a number, such as
`pick {x = "a", y = 1}`, is refused just as a call that leaves `Elt` with no
answer is. `Key` is bounded by `Any`, so it takes anything — every name in a
dict has a bound, and `Any` is what a name in a list is bounded by.

A bound is one type, written as a type name or sigiled, so a union is
`#{Elt: :(Number | Str)}`. It may not name another type parameter, and it may
not be `Never`, which no value could ever satisfy.

A signature's head parameter takes a bound the same way:

```koan
SIG Counter FOR ALL #{Carrier: Number} = #[(VAL zero :Carrier)]
MODULE ints = (
  (LET Carrier = Number)
  (LET zero = 0)
)
LET counter = (ints :| Counter)
```

A module fits `Counter` only when its `Carrier` works out to a type under
`Number`; one whose `zero` is a string is refused. The opaque view still hides
*which* type `Carrier` is, but not its bound: `counter.zero` is admitted by a
`:Number` slot and compares equal to `0`. With `FOR ALL #[Carrier]`, the view
would hide even that `zero` is an ordinary value. A `NEWTYPE` constructor's
parameters take no bound.

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

A meet must know both of its sides when the program loads, so neither may name a
`FOR ALL` variable or a signature's head parameter: in
`EXPR FOR ALL #[Elt] #(PICK x :(Elt & Number)) -> Elt`, each call picks its own
`Elt`, and the program is refused at the `&`. Bound the variable instead, with
`FOR ALL #{Elt: Number}`.

---

That completes the tour of the language as it stands. For the shape of what's
not built yet — arithmetic and comparison operators, loops, comments,
user-declared traits — see the project's roadmap. Back to the
[README](README.md) for the full chapter list.
