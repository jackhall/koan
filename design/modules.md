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
digest of each value and type it captures.

The code is the body as resolved, not its text. A keyworded use at a statement
sees only the definitions written above it
([keyworded uses](../src/scope/README.md#keyworded-uses)): after
`EXPR #(HELPER x :Any)`, `(HELPER 1)` calls that definition, and after a later
`EXPR #(HELPER x :Number)` the same text calls the new one. So two bodies of one
text at two places in a program may differ. A read of a top-level name
contributes the binding it reads, and a keyworded use the candidates it resolves
to: each is bound once per program, so it stands for its value, and the load
knows it without running anything.

Values are immutable, so a value's digest is a fact about it. It is computed
on demand, when a view asks for its source's, and composed from its parts'
digests, as a type's digest is composed from its children's; one demand
digests a part shared many times over once. A module is the exception: it
keeps no captures to digest later, so its content is digested where it is
built. Values that reach
one another — closures that call one another, a ring of containers a knot ties —
digest as one strongly connected component, as a recursive type group does.
Where a value sits in memory, which frame it was copied into, and the name a
module is bound under play no part.

Two modules of equal content run the same code over the same captured values, so
they hold the same invariants, and their values may be mixed freely. Two modules
that differ anywhere — an ascending and a descending `compare` — are two
modules. Two abstract types from one implementation, `Meters` and `Feet` over
one body, are therefore two modules that differ in content on purpose: by a
distinguishing member, which the load can see, or by a captured value, which
the run compares.

## The capture contract

`MODULE <name> OVER #[<names>] = (<body>)` names every outer value and type the
body may read, and the load refuses a body that reads an outer name the list
leaves out. A `GROUP` body is a module body, and its list also follows the
name: `GROUP <name> OVER #[<names>] FOLD LEFT = (<body>)`. The list therefore
covers everything that can make two evaluations of one body differ, and a
reader sees it at the binder: changing what a module captures changes its
types, and the contract makes that change visible where the module is written.

An outer name is one the body reads that neither the body nor a top-level
statement binds: a callable's parameter or local, or a name an enclosing module
body, `USING … SCOPE` block or `MATCH` arm binds. In a quote's code, a hole the
code leaves open is no outer name: what fills it decides it, as a top-level
binding decides a top-level read, and the module's content composes the value
it is filled with. A top-level name or a builtin
is bound once per program, so it is read where it lives and need not be
listed. A registration is captured as a name is
([keyworded uses](../src/scope/README.md#keyworded-uses)), and the list names
one by its key with `_` in each slot, `OVER #[x (HELPER _)]`, as a code
parameter's `NEEDING` does. A type an enclosing callable binds — a `FOR ALL`
variable, which each call solves — is listed as a value is, `OVER #[x Elt]`,
and the digest composes the type it is bound to. The names an `EVAL` in the body
offers are reads at the `EVAL`'s statement, so they fall under the list like any
other read. A body with no list captures nothing.

Every entry is part of the module's content, whether or not the body reads it,
so a list can brand a module: `MODULE meters OVER #[metric]` and
`MODULE feet OVER #[imperial]` over one body that reads neither name are two
modules. A top-level entry counts by its binding, as a top-level read does: a
module's content takes the set of top-level bindings its body reads or lists, so
listing one the body already reads changes nothing.

## Carriers and paths

An opaque ascription, `m :| Counter`, hides each head parameter the application
leaves unpinned behind a **carrier**. A carrier is keyed on content — the
ascribed module's, the signature application's and the parameter's name — and
never on when or how often the ascription is evaluated. A transparent
ascription, `m :! Counter`, shows what each parameter solves to.

A carrier hides the parameter's bound too. Under
`SIG Counter FOR ALL #{Carrier: Number}`, the bound decides which modules fit,
so a module whose `zero` is a string is refused, and nothing more: outside the
view `m.zero` is opaque, taken by no slot but one naming its carrier and equal
only to a value sealed under that carrier. Only the view's own functions, behind
their barriers, see the number it seals.

A **path type** names a type member through a module-valued expression:
`m.Carrier` through a name, `p.Carrier` through a parameter, `(MAKESET ints).Set`
through a call. Nothing is refused for its spelling. Where the load sees what the
member is — a body's binding, what `:!` solves, a pin — the path is that type. A
carrier, or a member of a module the load cannot see into, stands for whatever
the module carries, so `m.zero` and `m.succ` read through
one path are known at load to share a type, and a call solving one variable from
both solves it the same way at every run. What the load can say of two paths is
[below](#the-load-and-the-run).

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
log is an effect of the build, not a capture of the module. The load tells the
two apart by the function's effect row: a call whose row names no effect
[unfolds](#a-call-unfolds) into the content it builds, and one whose row
names an effect builds an action, so a module bound by running it is a leaf the
load cannot see into.

## The load and the run

The load and the run agree
([gradual typing](gradual-typing.md#the-load-and-the-run-agree)). The load
reads each module-valued expression as a **content tree** off the program text.
A tree's nodes are content, the same parts the run digests, and its leaves are
what the load cannot see:

| the expression | its tree |
|---|---|
| `MODULE n OVER #[x Elt] = (…)`, or a closure | a **body**: its resolved code over the trees of what it captures |
| `e :\| Counter`, `e :! Counter` | a **view**: the operator and the signature application over the tree of `e` |
| a value the load can compute, such as a literal | its digest |
| a name bound by `LET m = e` or `MODULE m = …` | the tree of what it is bound to |
| a member chain, `e.inner` | through a body, the tree of the member's binding |
| a call, `(MAKESET ints)` | the tree the function builds, [unfolded](#a-call-unfolds) |
| a parameter `p`, or a member chain through one, `p.inner` | a **leaf**, keyed on the binder and the chain |
| anything else | a leaf, keyed on the binder of the `LET` that names it |

A leaf no binder names — a `MATCH` written inline, a module read out of a list —
is unlike every tree, its own text written elsewhere included.

### A call unfolds

The load summarizes each function by the tree of the module its body returns,
its parameters as leaves. At a call it substitutes the arguments' trees for
those leaves and the call's solution for the function's type parameters, so
`(MAKESET ints)` is the tree `MAKESET` builds over `ints`. The load reads a
functor through its body, not its signature alone, as the run identifies a
module by its content.

A call that does not unfold is a leaf: one of a function a parameter holds or
that dispatch picks only at the run, one of a recursive function, or one whose
result is chosen by a `MATCH` or read out of a container. Bound by a `LET`, it
is keyed on that binder, so after `LET s = (BUILD p)`, `s.zero` and `s.succ`
share a type; written inline, it is unlike every tree.

### Comparing two paths

A path reads a member, so two paths are compared at their members. Where the
load sees what both members are, they compare as those types do. Otherwise one
rule reads the two trees:

| the two paths | at load |
|---|---|
| the same member of identical trees, each leaf keyed alike | *always* one type |
| two carriers whose trees hold content at one position and differ there — a body's code, a view's operator or signature, a digest — or that name two parameters of one view | *never* one type |
| otherwise | *maybe*, and the run compares carriers |

A tree with no leaf is a **constant** tree: the load computes the digest the run
keys the carrier on, so the load's type is the run's carrier. A tree with a leaf
gives a rigid type the run binds to the carrier it computes, read from outside
through its bound; it is a type of the load alone, and no value carries it. Both
follow the rule above, so every equality the load draws holds at the run, and a
*maybe* the run settles either way contradicts nothing the load said.

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

- [Path types](../roadmap/rewrite/path-types.md) — content trees, a path's
  load-time type, and a call unfolded.
- [Effects](../roadmap/rewrite/effects.md) — effect rows, composite effects, and
  a module bound from an action as a leaf.
