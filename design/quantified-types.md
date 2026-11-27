# Quantified types

Where a `FOR ALL` may be written, why it may be written nowhere else, and what
a program writes instead. Koan's polymorphism is on declarations: a quantified
function is called, and each call solves its group, or it is instantiated where
the type it is wanted at solves its group once. Anything that needs a
polymorphic function *as a value* — a higher-ranked slot, a polymorphic payload
— takes a module typed by a signature ([modules](modules.md)).

The rule exists because a parametric type is contagious. A quantified type let
into a concrete one weakens every type built over it
([the laws](../lattice/src/types/laws.md#a-values-type-is-concrete)), so the
design keeps one out of every type a value carries and gives it exactly one
sealed home.

## A `FOR ALL` lives on a declaration

Three declarations write one: a callable (`FN FOR ALL`, `EXPR FOR ALL`), a
signature's head (`SIG Stack FOR ALL #{Elt: Any}`), and a signature's member.
Everything else is refused, and each refusal reads syntax alone, so none needs
a type.

A callable's head also quantifies where its types read its own parameters:
`EXPR #(ECHO elem :Counter) -> elem.Carrier` and
`EXPR #(ECHO elem :Counter) -> :(TYPE OF elem)` each carry a **hidden
variable** in their group, which every call solves from the argument
([a head reading its parameters](../src/elaborate/README.md#a-head-reading-its-parameters)).
Such a head is quantified under every rule below, so a `FN` reading its
parameters is a quantified function, and a function type reading its own
parameters inside another type is refused as `Quantified`.

| Refusal | What it refuses | Owner |
|---|---|---|
| `QuantifiedRead` | a name a keyworded form binds, or a quantified member `USING … SCOPE` surfaces, read anywhere but the head of a call; a module's quantified member read through `$` in a quote or as an `EVAL` offer | [scope](../src/scope/README.md#resolution) |
| `QuantifiedValue` | a body whose last statement binds a quantified function by a keyworded form, which would be the body's value | [scope](../src/scope/README.md#resolution) |
| `Quantified` | a type expression `:(FN FOR ALL …)` or `:(EXPR FOR ALL …)` anywhere but as the whole type of a signature's `VAL` member or a signature's keyworded head, one nested inside an admitted one included | [elaborator](../src/elaborate/README.md#what-a-type-expression-is) |
| `MeetOverVariable` | `&` with an operand naming a `FOR ALL` variable or a head parameter | [elaborator](../src/elaborate/README.md#what-a-type-expression-is) |
| `Bound` | a bound that names a type variable, or is `Never` | [elaborator](../src/elaborate/README.md#what-a-type-expression-is) |

So a quantified function's type enters no other type. A binding keeps the
scheme in two places only: a `MODULE` or `GROUP` body's member,
`LET pick = (FN FOR ALL …)`, and a name a keyworded form binds. Anywhere else a quantified function is made
concrete ([below](#where-a-quantified-function-is-instantiated)), or refused.

In Rust the same rule is the handle types
([typed handles](../lattice/src/types/identity.md#typed-handles)). A quantified
callable's type is a `Scheme`, which is no child handle: no door that builds a
list, union, record or function type takes one. `Value::ktype` answers a
`DeclaredType`, a scheme for a quantified callable and a `KType` for every
other value, and every reader but a call's head takes `Value::concrete_ktype`.

## What a body may ask of a `FOR ALL` name

A body knows of its `FOR ALL` name what the bound declares, and nothing more.
A spelling over the name — a projection off it, an application with it as
head, a `MATCH … UNDER` clause naming it — loads only where the bound
guarantees it at every binding. A binding then writes a type in. It never
decides whether a spelling is a type, so no call is refused for a spelling
inside a body its caller did not write.

| Spelling in a body | `Elt`'s bound | The load |
|---|---|---|
| `Elt.size` | a record type declaring `size` | reads the [projection](../lattice/src/types/vocabulary.md#the-node-vocabulary) `(FIELD size OF Elt)`: each binding's `size`, and the union of the members' where a binding is a union |
| `Elt.size` | any other | refuses: the bound declares no `size` |
| `Elt.None` | any | refuses; under `Elt: Maybe` the variant is spelled `Maybe.None` |
| `MATCH x UNDER Elt` | any | refuses; under `Elt: Maybe` the clause is spelled `UNDER Maybe` |
| `Number -> Elt`, `Elt {…}`, `Elt WITH {…}`, `Elt NEEDING #[…]`, `UNBUILT Elt` | any | refuses |

A variant read and an `UNDER` clause have no reading that holds at every
binding. A binding under `Maybe` may be the lone variant `Some`, which has no
`None` and is no union, and a variant does not know its union
([a union's variants](../src/elaborate/README.md#a-unions-variants)). The
bound's own spelling says the same thing at every binding. No bound licenses a
head, since the type a value carries is always applied.

A projection is the one spelling that keeps a record value whole and names a
field's type:

```koan
EXPR FOR ALL #{Elt: :{size :Any}} #(RESIZER x :Elt) -> Any = #(
  (FN :{n :(Elt.size)} -> Elt = #(x)))
LET grow = (RESIZER {size = 5, width = 2})
PRINT (grow {n = 6})
```

```text
{size = 5, width = 2}
```

Quantifying the field instead, `x :{size :Size}`, re-stamps the argument at
`:{size :Size}`, and no bound may name `Size`.

The value side agrees. `x.size` over `x :Elt` is typed by the same projection,
so `LET n :(Elt.size) = (x.size)` holds at load, and a read of a field the
bound does not declare is refused at load
([static types](../src/dispatch/README.md#static-types)). A body that reads a
field only where the value has one tests the value:

```koan
EXPR FOR ALL #[Elt] #(SIZEOR x :Elt) -> Any = #(
  MATCH x WITH #{:(:{size :Any}): (it.size), _: (0)})
PRINT (SIZEOR {size = 5, width = 2})
PRINT (SIZEOR 7)
```

```text
5
0
```

A name with no declared bound licenses nothing: a `:Type` parameter, a name
`USING … SCOPE` surfaces that its module's signature leaves abstract, a
quote's hole or `$` name, and a path over a head parameter its signature
leaves unpinned. A surfaced name its module's static signature declares
exactly — a manifest member, or a head parameter the signature's application
pins — is that closed type. A type declared in a body over a `FOR ALL` name,
`UNION Chain = #{Link: :{v :Elt}, …}`, is no run-bound name: its declaration
licenses what it shows, each spelling checked at load against it. A value declared `Any` is the
declared way to leave a read to the call
([what a bind site binds](binding-graph.md#what-a-bind-site-binds)).

The rule is about what a spelling is. Whether a check over the name holds, and
which candidate a use selects, are a binding's to answer
([the load and the run agree](gradual-typing.md#the-load-and-the-run-agree)).

## Where a scheme is held

- **A callable's own type**: a function value's memo, and a registered shape.
  A call's head reads it, and the call solves its group; an instance site reads
  it, and the load solves its group.
- **A signature member**: a keyworded head with a `FOR ALL` of its own, or a
  `VAL` member whose whole type is a quantified function type.
- **Lists the interpreter holds and no name reaches**: a registration bucket,
  and the candidates a `USING` hole or an `EVAL` offer gathers at one key. A
  candidate list is laid down typed `List<Any>` without reading its functions'
  types ([values](../src/values/README.md#the-type-memo-and-satisfies)), and
  the load reads it as unknown. A container a program can read holds concrete
  types only.

## Where a quantified function is instantiated

An **instance site** is a quantified function, a name bound to one, a
module's quantified member read through the module, `m.pick`, or a
`FN FOR ALL` literal, written anywhere but the head of a call or a `MODULE` or
`GROUP` body's binding. Its group is solved there from the type it is **wanted**
at, and the value read there is the **instance**: a function stamped with the
concrete type its group was solved at, which is stored, passed, returned and
dispatched on like any unquantified function. A type is wanted at:

- an annotated binding's right-hand side, `LET inc :(FN :{x :Number} -> Number) = pick`;
- an ascription's operand, `pick :! :(FN :{x :Str} -> Str)`;
- a callable body's last statement, at the body's declared return;
- a call by name's argument, at the callee's parameters;
- a keyworded argument, at the slot of each candidate the use keeps;
- a list, dict or record literal's parts, at that container's own type.

A plain `LET` holds a generic value, in any body: `LET pick = (FN FOR ALL …)`
binds the scheme, and each call of `pick` by name solves its group at the
call. A `LET` whose value its body returns — a callable body's, an arm's or a
quote's last statement — is an instance site wanted at the body's declared
return, and an annotated `LET` one wanted at its annotation, so
`LET inc :(FN :{x :Number} -> Number) = (FN FOR ALL …)` binds the instance.

A member read `m.pick` is an instance site where the load sees that the
member is quantified: its operand is a name, or a chain of member reads rooted
at one, whose static type is a signature declaring the member with a scheme.
A scheme naming a head parameter that signature leaves unpinned reads that
parameter as the [path](modules.md#carriers-and-paths) through the operand,
`m.Carrier`, and is instantiated there like any other: with `m :Mapper`
declaring `VAL apply :(FN FOR ALL #[Elt] :{x :Carrier, y :Elt} -> Elt)`,
`LET go :(FN :{x :(m.Carrier), y :Number} -> Number) = m.apply` loads. Only a
read whose path the load cannot name — one reaching across a quote's code — is
refused anywhere but a call's head, where the run solves the call. A read
the load cannot see into, off a module whose signature only the run gives, is
an instance site too, answered where the module binder is bound: its scheme is
what the binding gives.

The solve is the least instance of the scheme under the wanted type
([`instance_under`](../lattice/src/types/relations.md#quantified-binders)). A
variable no contribution from the wanted type reaches is refused and named,
never read as its bound; one some contribution reaches binds its least
instance, so `:(FN :{x :Number} -> Any)` fixes `Elt` to `Number`. A
quantified callee's other arguments solve its group first, a variable a class
ranked before the slot's solves taken as the call solves it, and the slot is
read through that solve. The candidates the text leaves a keyworded use must
agree on each instance, each name declared `Any` read at its ends: a binding of
an `Any` name does not choose a member argument's instance.

The instance is made where the solve that fixes it is decided
([where a solve is decided](binding-graph.md#where-a-solve-is-decided)):

- by the load, where the wanted type is closed;
- at the type each run binds, where the wanted type names a type a run binds,
  such as an enclosing `EXPR FOR ALL #[Outer]`'s `Outer`, whether the lattice
  spells the solution over `Outer` or each binding of `Outer` solves it;
- where a module binder is bound, for a member the load cannot see into: the
  scheme is what the binding gives, and its instance is what the selection
  over it answers.

A site nothing wants at a type, one wanted at a union or `Any`, one whose
solving argument is itself an instance site, and one with an argument at most
`Any` among those that solve its variables are refused at the load. The last
asks for a `FOR ALL` name: the text may use a type only where it declares one
([what a bind site binds](binding-graph.md#what-a-bind-site-binds)).
[Dispatch](../src/dispatch/README.md#static-types) owns the rule and its
refusals.

## How a signature keeps a scheme safe

A signature is the one type that holds a scheme, and it is concrete. Four
rules make that sound.

**A signature is sealed.** It is a leaf to every walk, probe and substitution,
so nothing reads the scheme inside it as part of the type around it. A head
parameter is reached through an application's pins, never by entering the
signature.

**The order never looks at members.** A signature type is a set of
[applications](../lattice/src/types/relations.md#signature-types), ordered by
their pins alone, each pin at an *equal* type. `Stack WITH {Elt = Number}`
lies under `Stack`; two different pins are unordered, even `Number` and
`Number | Str`, since a head parameter may sit at both variances across
members. Whether one signature's members cover another's is never an order
question, so the order stays a partial order by handle with schemes inside.

**Members are compared by *fits*, which reads a variable by its side.**

| | offered side | asked side |
|---|---|---|
| a member's own `FOR ALL` variable | solved afresh for each member, as a call solves it | rigid |
| an unpinned head parameter | one rigid unknown per application, opaque | one variable, solved and discarded |
| a pinned head parameter | the pin | the pin |

An offered head parameter's bound decides only which modules fit an asked
one's bound, and reveals nothing else, as an opaque view's carrier does
([relations](../lattice/src/types/relations.md#signature-types)).

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
| A generic function passed or stored at one type | the function where a type is wanted that solves its group: `LET inc :(FN :{x :Number} -> Number) = pick`, `[pick] :! :(LIST OF (FN :{x :Number} -> Number))`, a slot typed `:(FN :{x :Number} -> Number)` |
| A generic function bound outside a module | the binding under a type that solves its group: `LET inc :(FN :{x :Number} -> Number) = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))` |
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
- **A bound is closed.** In the lattice it names no variable and no head
  parameter, so no variable is bounded by another, and an opaque carrier
  bounds none. A callable's own `FOR ALL` group may name an encloser's name:
  its static type carries the scheme's recipe, the bound written over names,
  and the scheme is built with the bound closed where the callable is born
  ([the binding graph](binding-graph.md#what-a-bind-site-binds)). A bound
  inside a type, a `SIG`'s `VAL` scheme or its own group, names no
  encloser's name, and is refused at the load.
- **No meet over a variable.** Each call solves the variable, so the meet
  cannot be taken at load, and no intersection type keeps it symbolic.
- **A body asks only what the bound declares.** A projection off a `FOR ALL`
  name needs a bound declaring the field, and no bound licenses a variant
  read, an `UNDER` clause or a head
  ([above](#what-a-body-may-ask-of-a-for-all-name)).
- **No union whose members tie.** A union in a declaration's parameter, slot
  or variant is admitted through its most specific member, so
  `(Elt | (LIST OF Key))` over `[1]` binds `Key`. Two members neither of which
  is more specific, that one argument may both admit — `(Elt | Key)` with equal
  bounds, `(:{a :Elt} | :{b :Key})` — would solve the group by how the union
  stores them, and the declaration is refused (`TiedUnion`,
  [elaborator](../src/elaborate/README.md#refusals)).
- **A nominal type is never generic over a run-bound name.** A `NEWTYPE` over
  a `FOR ALL` name is a different type each call, since substitution never
  enters a sealed nominal.
- **A surfaced head is typed only through a closed signature.** Under a rigid
  ascription or a meet, the `USING … SCOPE` registration is unknown to the
  load, and each use is decided by the call.
- **An instance is made only where the program loads.** A site whose wanted
  type the load does not know is refused; nothing is solved at run time.
- **A wanted type is a function type itself.** A union, `Any` or anything else
  fixes nothing, and is never split. A container literal passes a type to its
  parts only where that type is the container's own, and a keyworded argument
  takes one only where it is itself an instance site: `KEEP [pick]` is refused,
  and `KEEP ([pick] :! :(LIST OF (FN :{x :Number} -> Number)))` loads.
- **A keyworded form's function is call-only.** The name `LET id = FN EXPR FOR ALL …`
  binds, a quantified member `USING … SCOPE` surfaces, a `$pick` and an `EVAL`
  offer stand only at the head of a call.

## In the literature

Each rule above is a known one, and the source says what it buys and costs.

| Koan | Known as | Source |
|---|---|---|
| `KType` and `Scheme`; a `FOR ALL` on a declaration, solved at each use | types and type schemes; let-polymorphism | Damas and Milner, *Principal type-schemes for functional programs* (1982) |
| A call solving its group, and an instance site | a scheme is instantiated at each use and is no first-class value | the same |
| An instance site's type read from the type it is wanted at | checking mode: type arguments propagated from the expected type | Pierce and Turner, *Local type inference* (2000) |
| A rank-2 slot spelled as a signature with a quantified member | higher-rank polymorphism packaged in a module or a record value with a polymorphic field | Russo, *First-class structures for Standard ML* (2000); Rossberg, *1ML* (2015) |
| *Fits*' table: the offered side solved afresh, the asked side rigid | subsumption between polymorphic types: instantiate the offered type, skolemize the asked one | Mitchell, *Polymorphic type inference and containment* (1988); Peyton Jones, Vytiniotis, Weirich and Shields, *Practical type inference for arbitrary-rank types* (2007) |
| A sealed signature; a variable that never leaves its binder | the skolem escape check | the same |
| *Fits* answered by a terminating, incomplete solve | containment by instantiation is undecidable | Tiuryn and Urzyczyn, *The subtyping problem for second-order types is undecidable* (1996) |
| A variable under a declared bound | bounded quantification, whose subtyping is undecidable in its full generality | Pierce, *Bounded quantification is undecidable* (1992) |
| A head parameter; `WITH`; a manifest member | abstract and manifest type components; `with type` | Harper and Lillibridge, *A type-theoretic approach to higher-order modules with sharing* (1994); Leroy, *Manifest types, modules, and separate compilation* (1994) |
| `:\|` and its carrier, keyed on content; `:!` | opaque (generative) and transparent ascription; sealing as an existential | Rossberg, Russo and Dreyer, *F-ing modules* (2014) |
| An unpinned head parameter: one rigid unknown offered, one solved variable asked | an existential type, packed on one side and opened on the other | the same |

Two departures matter when reading those sources. Koan solves a group at the
call, from the upper end of each argument's static type and from the type it
*carries* where the load knows nothing of it, each read at the types the run
binds, so a solve always has concrete types to read and never infers a
scheme. And its `Any` is the top of a
lattice, not an unknown type, so nothing here is checked by consistency.

## Open work

- [A head reading its parameters' types](../roadmap/types/head-parameter-reads.md)
  — the hidden variable a head's read of its own parameters introduces.
- [A check over a `FOR ALL` name holds at every binding](../roadmap/types/checks-at-every-binding.md)
  — a body's checks held to the bound as its spellings are.
- [A check settled arm by arm](../roadmap/types/checks-by-arm.md) — a check
  that holds on every arm of the selection it reads.
- [A `FOR ALL` name bounded from below](../roadmap/types/bounds-from-below.md)
  — a body returning a value it builds at the name.
- [Unplanned work](../roadmap/types/README.md#unplanned-work) — a head
  projecting a `FOR ALL` name, and a use's candidates checked for coverage at
  load.
