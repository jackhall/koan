# Modules

What a module is, what its types are, and when two of its types are one.

A module is a value. A `MODULE` body or an ascription builds one, and it is
passed, stored and returned like any other value. A functor is an ordinary
function whose body returns a module, so no part of the module system is
confined to a static layer of the program. The pieces live in their own
modules: [the module layer](../src/knot/module/README.md) builds and reads a
module, [the type lattice](../lattice/src/types/identity.md) digests a type, and
[values](../src/values/README.md#the-type-memo-and-satisfies) carry what a value
is.

## A module is its content

A module's identity is a digest of its content: the code of its body and the
digest of each value it captures. Values are immutable, so a value's digest is a
fact about it. It is memoized where the value is laid down, as its type is, and
composed from its parts' digests, as a type's digest is composed from its
children's. Closures that call one another digest as one strongly connected
component, as a recursive type group does. Where a value sits in memory, and
which frame it was copied into, play no part.

Two modules of equal content run the same code over the same captured values, so
they hold the same invariants, and their values may be mixed freely. Two modules
that differ anywhere — an ascending and a descending `compare` — are two
modules.

## The capture contract

`MODULE <name> OVER #[<names>] = (<body>)` names every outer value the body may
read, and the load refuses a body that reads an outer name the list leaves out.
The list is therefore everything that can make two evaluations of one body
differ, and a reader sees it at the binder: changing what a module captures
changes its types, and the contract makes that change visible where the module
is written.

## Carriers and paths

An opaque ascription, `m :| Counter`, hides each head parameter the application
leaves unpinned behind a **carrier**. A carrier is keyed on content — the
ascribed module's and the signature application's — and never on when or how
often the ascription is evaluated. A transparent ascription, `m :! Counter`,
shows what each parameter solves to.

A **path type** names a type member through a module: `m.Carrier`. The load
reads it as a rigid type standing for whatever the module bound to `m` carries,
so `m.zero` and `m.succ` read through one path are known at load to share a
type, and a call solving one variable from both solves it the same way at every
run.

## Applicative by content, generative by effect

Koan has no switch between generative and applicative modules. A functor
applied twice to arguments of equal content builds modules of equal content, so
their types are one: `(MAKESET ints).Set` names one type wherever it is written.
Two applications differ in their types exactly where the modules they build
differ in content — a different argument, or a different captured value.

Every effect is a monadic value, so a fresh value comes only from an effect. A
functor that must mint new types at each use — a symbol table whose keys index
only its own table — draws a fresh value from an effect and captures it, and
each run of that action builds a module of new content. A functor that only
logs while it builds leaves its module's content, and its types, unchanged: the
log is an effect of the build, not a capture of the module.

## The load and the run

The load and the run agree
([gradual typing](gradual-typing.md#the-load-and-the-run-agree)). A path the
load reads twice holds one module at every run, and a carrier is keyed on that
module's content, so every equality the load draws between types read through
paths holds at the run. Two different paths may hold modules of equal content at
a run, and the run then gives them one carrier.

## In the literature

| Koan | Known as | Source |
|---|---|---|
| A functor's result types fixed by the functor and its argument | applicative functors | Leroy, *Applicative functors and fully transparent higher-order modules* (1995) |
| A pure application applicative, an effectful one generative | purity decides between applicative and generative functors | Rossberg, Russo and Dreyer, *F-ing modules* (2014); Dreyer, *Understanding and evolving the ML module system* (2005) |
| `m.Carrier` | path-dependent types | Leroy, *Manifest types, modules, and separate compilation* (1994); Amin, Rompf and Odersky, *Foundations of path-dependent types* (2014) |
| A fresh type only through an effect | the rank-2 state thread of `runST` | Launchbury and Peyton Jones, *Lazy functional state threads* (1994) |
| Identity by content digest | content-addressed definitions | the Unison language |

One departure matters when reading those sources. OCaml keys an applicative
functor's result on the argument's *path*, so two separately built modules of
equal content give two types; koan keys it on content, so they give one.

## Open work

- [Modules](../roadmap/rewrite/modules.md) — content digests, carriers keyed on
  them, path types, the capture contract, and evaluating module programs.
- [Effects](../roadmap/rewrite/effects.md) — effect rows, composite effects, and
  a path through an application.
