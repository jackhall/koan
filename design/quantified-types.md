# Quantified types

Where a `FOR ALL` may be written, why it may be written nowhere else, and what
a program writes instead. Koan's polymorphism is on declarations: a quantified
function is called, and each call solves its group. Anything that needs a
polymorphic function *as a value* — a higher-ranked slot, a polymorphic payload
— takes a module typed by a signature.

The rule exists because a parametric type is contagious. A quantified type let
into a concrete one weakens every type built over it
([the laws](../src/type_lattice/laws.md#a-values-type-is-concrete)), so the
design keeps one out of every type a value carries and gives it exactly one
sealed home.

## A `FOR ALL` lives on a declaration

Three declarations write one: a callable (`FN FOR ALL`, `EXPR FOR ALL`), a
signature's head (`SIG Stack FOR ALL #{Elt: Any}`), and a signature's member.
Everything else is refused, and each refusal reads syntax alone, so none needs
a type.

| Refusal | What it refuses | Owner |
|---|---|---|
| `QuantifiedLambda` | a quantified `FN` written anywhere but a binder's right-hand side or the head of a call | [scope](../src/scope/README.md#resolution) |
| `QuantifiedRead` | a name bound to a quantified function, read anywhere but the head of a call by name — in a list, as an argument, through `$` in a quote, as an `EVAL` offer | [scope](../src/scope/README.md#resolution) |
| `QuantifiedValue` | a body whose last statement binds a quantified function, which would be the body's value | [scope](../src/scope/README.md#resolution) |
| `Quantified` | a type expression `:(FN FOR ALL …)` or `:(EXPR FOR ALL …)` anywhere but as the whole type of a signature's `VAL` member or a signature's keyworded head, one nested inside an admitted one included | [elaborator](../src/elaborate/README.md#what-a-type-expression-is) |
| `MeetOverVariable` | `&` with an operand naming a `FOR ALL` variable or a head parameter | [elaborator](../src/elaborate/README.md#what-a-type-expression-is) |
| `Bound` | a bound that names a type variable, or is `Never` | [elaborator](../src/elaborate/README.md#what-a-type-expression-is) |

So a quantified function is **call-only**: every binding of one — a `LET`, a
module member, a name `USING … SCOPE` surfaces — stands only at the head of a
call, and its type enters no other type.

In Rust the same rule is the handle types
([typed handles](../src/type_lattice/identity.md#typed-handles)). A quantified
callable's type is a `Scheme`, which is no child handle: no door that builds a
list, union, record or function type takes one. `Value::ktype` answers a
`DeclaredType`, a scheme for a quantified callable and a `KType` for every
other value, and every reader but a call's head takes `Value::concrete_ktype`.

## Where a scheme is held

- **A callable's own type**: a function value's memo, and a registered shape.
  Only a call's head reads it, and the call solves its group.
- **A signature member**: a keyworded head with a `FOR ALL` of its own, or a
  `VAL` member whose whole type is a quantified function type.
- **Lists the interpreter holds and no name reaches**: a registration bucket,
  and the candidates a `USING` hole or an `EVAL` offer gathers at one key. A
  candidate list is laid down typed `List<Any>` without reading its functions'
  types ([values](../src/values/README.md#the-type-memo-and-satisfies)), and
  the load reads it as unknown. A container a program can read holds concrete
  types only.

## How a signature keeps a scheme safe

A signature is the one type that holds a scheme, and it is concrete. Four
rules make that sound.

**A signature is sealed.** It is a leaf to every walk, probe and substitution,
so nothing reads the scheme inside it as part of the type around it. A head
parameter is reached through an application's pins, never by entering the
signature.

**The order never looks at members.** A signature type is a set of
[applications](../src/type_lattice/relations.md#signature-types), ordered by
their pins alone, each pin at an *equal* type. `Stack WITH {Elt = Number}`
lies under `Stack`; two different pins are unordered, even `Number` and
`Number | Str`, since a head parameter may sit at both variances across
members. Whether one signature's members cover another's is never an order
question, so the order stays a partial order by handle with schemes inside.

**Members are compared by *fits*, which reads a variable by its side.**

| | offered side | asked side |
|---|---|---|
| a member's own `FOR ALL` variable | solved afresh for each member, as a call solves it | rigid |
| an unpinned head parameter | one rigid unknown per application | one variable, solved and discarded |
| a pinned head parameter | the pin | the pin |

So a module defining `EXPR FOR ALL #[Elt] #(BOX x :Elt) -> :(LIST OF Elt)`
fits `SIG Boxes = #[(EXPR FOR ALL #[Elt] #(BOX _ :Elt) -> :(LIST OF Elt))]`,
and one defining `BOX` only at `Number` does not: `Number` is not the rigid
`Elt`. The asked side is the promise a reader of the slot relies on, and it
must hold at every instance.

**Three choices keep *fits* transitive.**

- For each asked keyworded member, one offered overload contributes to the
  head parameters' solve, each tried in turn. Pooling a key's overloads would
  refuse a module at `Stack` that fits the meet of `Stack`'s two pinned
  applications, which lies under `Stack`.
- A tie between two offered overloads is not refused. It is an ambiguity where
  a call meets it.
- An offered member with a group of its own contributes nothing to the head's
  solve and is checked after it.

A module's self-signature stands on the offered side like any signature. On
the asked side it compares by handle: asking for one is asking for that
module's exact content.

## What a program writes instead

| Wanted | Written |
|---|---|
| A generic function passed or stored at one type | an unquantified `FN` that calls it: `(FN :{x :Number} -> Number = #(pick {x = x}))` |
| A slot taking a function usable at several types (a rank-2 parameter) | a slot typed by a signature with a quantified member, `m :Boxes`, taking a module |
| Several instances used in one body | `USING (m :! Boxes) SCOPE (…)`: each keyworded use of a surfaced head solves the member's variables afresh, so `BOX 1` is at most `LIST OF Number` and `BOX "s"` at most `LIST OF Str` through the one `m` |
| A polymorphic function held in data | a module held in data, typed by its signature |
| One type per module, hidden or shown | a head parameter: `Stack WITH {Elt = Number}` pins it, `:\|` hides it behind a carrier |
| A variable each use solves | a `FOR ALL` on the member, not the head; position says which, so a per-use variable takes no pin |
| A polymorphic member called by name | a `VAL` member typed by a quantified function type, read only at the head of a call |

## Limits

- **A member's variable solves to anything**, a signature holding a quantified
  member included. Containment by instantiation is undecidable in general, so
  *fits* is what the unifier's collector answers. It terminates, and it is
  incomplete: a relation it misses is refused, at load and at run alike, so a
  miss is never unsound.
- **The search is a product.** With a head parameter unpinned, *fits* tries
  one offered overload per asked keyworded member, so the worst case is the
  product of the overload counts at each asked key. Fully pinned, nothing is
  searched.
- **Nesting is shallow.** An `EXPR` type nested in a quantified head opens an
  empty group of its own, and a name only the outer group declares is refused
  under it. A bare `FN` type opens no group and keeps reading the enclosing
  head's variables.
- **A bound is closed.** It names no variable and no head parameter, so no
  variable is bounded by another, and an opaque carrier bounds none.
- **No meet over a variable.** Each call solves the variable, so the meet
  cannot be taken at load, and no intersection type keeps it symbolic.
- **A nominal type is never generic over a run-bound name.** A `NEWTYPE` over
  a `FOR ALL` name is a different type each call, since substitution never
  enters a sealed nominal.
- **A surfaced head is typed only through a closed signature.** Under a rigid
  ascription or a meet, the `USING … SCOPE` registration is unknown to the
  load, and each use is decided by the call.
- **A quantified function is instantiated only by a call.** No ascription,
  annotation or declared return solves its group.

## In the literature

Each rule above is a known one, and the source says what it buys and costs.

| Koan | Known as | Source |
|---|---|---|
| `KType` and `Scheme`; a `FOR ALL` on a declaration, solved at each use | types and type schemes; let-polymorphism | Damas and Milner, *Principal type-schemes for functional programs* (1982) |
| Call-only, and the wrapping `FN` | a scheme is instantiated at each use and is no first-class value; the wrapper is an eta-expansion at one instance | the same |
| A rank-2 slot spelled as a signature with a quantified member | higher-rank polymorphism packaged in a module or a record with a polymorphic field | Russo, *First-class structures for Standard ML* (2000); Rossberg, *1ML* (2015) |
| *Fits*' table: the offered side solved afresh, the asked side rigid | subsumption between polymorphic types: instantiate the offered type, skolemize the asked one | Mitchell, *Polymorphic type inference and containment* (1988); Peyton Jones, Vytiniotis, Weirich and Shields, *Practical type inference for arbitrary-rank types* (2007) |
| A sealed signature; a variable that never leaves its binder | the skolem escape check | the same |
| *Fits* answered by a terminating, incomplete solve | containment by instantiation is undecidable | Tiuryn and Urzyczyn, *The subtyping problem for second-order types is undecidable* (1996) |
| A variable under a declared bound | bounded quantification, whose subtyping is undecidable in its full generality | Pierce, *Bounded quantification is undecidable* (1992) |
| A head parameter; `WITH`; a manifest member | abstract and manifest type components; `with type` | Harper and Lillibridge, *A type-theoretic approach to higher-order modules with sharing* (1994); Leroy, *Manifest types, modules, and separate compilation* (1994) |
| `:\|` and its carrier, minted per ascription; `:!` | opaque (generative) and transparent ascription; sealing as an existential | Rossberg, Russo and Dreyer, *F-ing modules* (2014) |
| An unpinned head parameter: one rigid unknown offered, one solved variable asked | an existential type, packed on one side and opened on the other | the same |

Two departures matter when reading those sources. Koan solves a group from
the types the arguments *carry*, at the call, so a solve always has concrete
types to read and never infers a scheme. And its `Any` is the top of a
lattice, not an unknown type, so nothing here is checked by consistency.

## Open work

- [Instantiating a quantified function](../roadmap/gradual-typing/instantiating-quantified-functions.md)
  — a quantified function made concrete by a solve, an annotation or an
  ascription, and where one may be bound at all.
- [Modules](../roadmap/rewrite/modules.md) — carriers keyed on their root, a
  carrier as a bound, higher-kinded head parameters, `m.f` outside the head of
  a call, and calling through an opaque view's quantified member.
