# Gradual typing

A koan program is typed as far as its declarations reach, where it loads, and
checked by each call for the rest. A program that states its types is refused
early where they decide it; one that leaves them out still runs, and each call
checks what the load could not.

This doc tells that story across the modules that carry it. Each owns its own
part: [the type lattice](../src/type_lattice/README.md) holds the types and the
relations between them, [the elaborator](../src/elaborate/README.md) reads a
written type into a handle, [dispatch](../src/dispatch/README.md) types
expressions and selects candidates, [values](../src/values/README.md) carry
types, and [the body runner](../src/program/README.md#the-body-runner) binds
and retypes at a call.

## Two kinds of type

A type is **concrete** or **parametric**, and the difference is the root of
the design.

- A **concrete** type is one a value can carry: `Number`,
  `LIST OF (Number | Str)`, `:{x :Number}`, a newtype, a module's signature.
  Concrete types form a lattice: one order, with a least upper bound (`join`)
  and a greatest lower bound (`meet`) for every pair.
- A **parametric** type holds a variable no one has solved yet: the `Elt` of a
  `FOR ALL #[Elt]`, a signature's head parameter, or a name only a run binds.
  The type of a quantified callable is a **scheme**: a type over a `FOR ALL`
  group of its own.

A variable is not a type a value has. It stands for a concrete type that a
solve will supply, and until then it is known only by two ends: it lies at or
above a lower end and at or under its declared bound. It is never read as its
bound. Under

```koan
EXPR FOR ALL #[Elt] #(ONLY x :Elt) -> Elt = #(x)
```

`ONLY 1` is a `Number`, not an `Any`: the call solves `Elt` from what it is
given.

So parametric types never enter the lattice's order. They are related by
[*fits*](../src/type_lattice/relations.md#the-relations), which solves: a
scheme fits another type when some instance of it lies under that type. In
Rust the split is three handle types — `KType`, `Parametric` and `Scheme` —
and passing one for another is a compile error
([typed handles](../src/type_lattice/identity.md#typed-handles)).
[The laws](../src/type_lattice/laws.md) says what each side of the split
guarantees and what breaks when the two are mixed, and
[quantified types](quantified-types.md) says where a `FOR ALL` may be written
and what a program writes where it may not.

## Three moments

A type is settled at one of three moments, each solving what the one before
left open.

### Where a type is written

[The elaborator](../src/elaborate/README.md#what-a-type-expression-is) reads a
type expression into a handle. A `FOR ALL` name reads as its variable, so a
callable's declared type is parametric, or a scheme where it declares a group.
A spelling that needs a concrete operand — `&`, an application's head — and is
given a variable is refused or left to the run, since its value could differ
at each call.

### Where the program loads

Two passes run before anything else does.

[The type channel](../src/elaborate/README.md#the-type-channel-at-load) types
every written type. Each comes out **closed** (the same at every run),
**rigid** (it names a type a run binds, held as a lexical variable), or
**unknown** (left to the run).

[The static-type pass](../src/dispatch/README.md#static-types) then gives
every value expression a **static type**: an *interval* of types within which
whatever a run carries there must lie. A literal is exact — its two ends meet.
A parameter declared `:(Number | Str)` is at most that. An expression nothing
ascribes is at most `Any`. A generic call is solved over its arguments'
intervals to an interval per variable, and its return is read through them.

Each candidate of each keyworded call then gets a verdict:

| Verdict | Means | The load |
|---|---|---|
| *never* | no call within the arguments' static types is admitted | drops the candidate; a use left with none is refused |
| *always* | every such call is admitted | may select the winner, which the call then runs without admitting or ranking |
| *maybe* | anything else | leaves the candidate to the call |

The same reading refuses a body that can never satisfy its declared return, an
ascription or an annotated binding that can never hold, and a call by name of an
exactly known function its argument can never satisfy; it settles an ascription
or annotation that always holds:

```text
no overload of `_ + _` admits (Str, Number)
this body returns Number, which can never satisfy its declared return Str
```

A quantified function written anywhere but the head of a call or a `MODULE` or
`GROUP` member's binding is instantiated where the load reads it, at the type it is
wanted at there — an annotation, an ascription, a declared return, a slot — or
refused where that type fixes nothing
([quantified types](quantified-types.md#where-a-quantified-function-is-instantiated)).

### Where a call runs

Every value carries a concrete type, memoized where the value is laid down
([the type memo](../src/values/README.md#the-type-memo-and-satisfies)). A call
reads its arguments' carried types and does whatever the load left:

1. it **admits** each remaining candidate, solving a quantified candidate's
   group from the carried types
   ([the unifier](../src/type_lattice/solving.md#the-unifier-collects-it-does-not-bind));
2. it **ranks** the admitting candidates
   ([selection](../src/dispatch/README.md#selection));
3. it **binds** each variable to its least instance, a concrete type, and
   **retypes** each argument to its parameter's declared type and the result
   to the declared return.

A retype is what makes a declaration a contract at run time: after it, the
value's type is what the declaration says, at every depth a reader sees.

## The load never contradicts the run

Gradual typing is sound only under one rule: **a verdict the load reaches
holds at every run.** The load decides exactly what the declarations decide,
and never guesses.

- A static type is an interval, and every rule maps both ends. Reading only an
  upper end claims a value is exactly what it is at most.
- A lexical variable stands for whatever a run binds, so a relation over one
  must hold for every binding. The load compares a rigid type only along the
  lexical chain it was typed in, and reads it from outside through
  [`bound_above`](../src/type_lattice/solving.md#substitute-then-ask).
- Where a relation holds at some bindings and not others, the answer is
  *maybe* or *unknown*, and the call decides.

Under `EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str`, `PAIR 1 WITH 2`
is *always*, while `PAIR a WITH b` over parameters `a :(Number | Str)` and
`b :(Number | Str)` is *maybe*: `a` may carry a `Number` where `b` carries a
`Str`.

Debug builds check the rule on every run: a finished value's carried type lies
within its node's static type, and a call the load decided runs what selection
over the full candidate list would.

## In the literature

| Koan | Known as | Source |
|---|---|---|
| A program typed where it states types and checked by the call elsewhere | gradual typing | Siek and Taha, *Gradual typing for functional languages* (2006) |
| The load refuses only what can never succeed (*never*), and guesses nothing | success typings | Lindahl and Sagonas, *Practical type inference based on success typings* (2006) |
| A static type as an interval of the types a run can carry | a gradual type read as the set of static types it stands for | Garcia, Clark and Tanter, *Abstracting gradual typing* (2016) |
| A variable solved from lower and upper contributions to its least instance | local type argument synthesis: collect bound constraints, pick the minimal solution | Pierce and Turner, *Local type inference* (2000) |
| A variable known by two ends; read from above or below by variance | polar types and bisubstitution | Dolan and Mycroft, *Polymorphism, subtyping, and type inference in MLsub* (2017) |
| Selection among overloads by the carried types, ranked by specificity | multiple dispatch over a subtype lattice with unions | Zappa Nardelli et al., *Julia subtyping: a rational reconstruction* (2018) |
| Polymorphism on declarations, modules for the rest | type schemes and the ML module system | [quantified types](quantified-types.md#in-the-literature) |

Koan differs from the gradual-typing sources in one respect that the rest of
the design follows from: `Any` is the lattice's top and a value always carries
a concrete type, so the run checks by the order and *fits*, never by a
consistency relation or a cast inserted at load.

It also does not aim at the *gradual guarantee* (Siek, Vitousek, Cimini and
Boyland, *Refined criteria for gradual typing*, 2015), under which removing an
annotation changes no result. A koan annotation is a contract a value is
retyped to, and dispatch selects by the type a value carries, so an ascription
changes what a reader sees and which overload runs.

## Where each piece lives

| Piece | Owner |
|---|---|
| Concrete and parametric handles, the order, *fits*, `join`, `meet` | [type lattice: relations](../src/type_lattice/relations.md), [identity](../src/type_lattice/identity.md) |
| The laws and what breaks without them | [type lattice: laws](../src/type_lattice/laws.md) |
| The solve, intervals, priority classes, verdicts | [type lattice: solving](../src/type_lattice/solving.md) |
| A written type read into a handle; closed, rigid, unknown | [elaborator](../src/elaborate/README.md) |
| Static types, static selection, instance sites, the return, ascription and annotation checks | [dispatch: static types](../src/dispatch/README.md#static-types) |
| Admission and ranking at a call | [dispatch: selection](../src/dispatch/README.md#selection) |
| A value's carried type, `satisfies`, retyping | [values: the type memo](../src/values/README.md#the-type-memo-and-satisfies) |
| Binding arguments and holding the return contract | [program: the body runner](../src/program/README.md#the-body-runner) |
| Where a load-time type is stored | [scope: load-time types](../src/scope/README.md#load-time-types) |

## Open work

- [Gradual typing](../roadmap/gradual-typing/README.md) — the project's open
  items.
- [Calls solved from their static types](../roadmap/gradual-typing/static-solutions.md)
  — a call's group solved from what the load knows of its arguments.
- [A container literal's element type](../roadmap/gradual-typing/container-literal-types.md)
- [A nested projection](../roadmap/gradual-typing/nested-projection.md)
