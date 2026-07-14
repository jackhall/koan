# Generic functions

Generic functions in koan are functors — module-returning functions — over their
type parameters, selected and
applied by modular-implicit resolution at the call site. There is no separate
free-type-parameter form: a type parameter is always an FN parameter, and so
always has a binder.

## Shape

A generic function is an FN taking one or more `:Type` parameters and
returning a module that holds the function specialized to those types:

```
LET make_head = (FN (MAKEHEAD Ty :Type) -> Module = (
  MODULE built = ((FN (HEAD xs :(LIST OF Ty)) -> Ty = ...))
))
```

Inside the body `Ty` is a bound parameter, so the inner FN is ordinary:
`Ty` resolves through the outer FN's per-call scope, the same path body-position
`er.Type` references use ([functors.md](functors.md)). No free type-parameter
name is introduced anywhere.

At a call site `HEAD some_list`, modular-implicit resolution
([implicits.md](implicits.md)) selects `make_head`, supplies its type argument,
applies it, and calls the produced `head`. Defining and invoking a generic needs
no syntax beyond the FN binder and an ordinary call.

## Why functors

- **One surface.** A type parameter is always an FN parameter — one spelling,
  always bound. A misspelled type name resolves to nothing and errors, rather
  than silently registering as a fresh parameter.
- **One engine.** A single resolution path handles every generic, whether or not
  it consults operations on its type parameter (see below). There is no separate
  binder for purely parametric functions running alongside the implicit resolver.
- **Hidden machinery.** The work of turning `HEAD some_list` into a specialized
  call lives behind the implicit/functor surface. Because callers never see it,
  the engine is free to short-circuit or memoize it without any change to
  user-visible syntax.

## Parametric and operation-bearing generics

The same surface and the same engine serve two kinds of generic; they differ only
in whether the selected module carries operations.

- **Parametric** — `head` returns the first element and consults nothing about
  the element type. It must work for element types that have no instances at all
  (the head of a list of function values, which have no `Ord` or `Eq`).
  Resolution supplies only the type argument; there is no witness to search for.
  This is the trivial case the resolver reduces to a direct read of the
  argument's carried element type.
- **Operation-bearing** — `sort` consults an ordering that is not contained in
  the element type. Resolution finds an `Ord` witness for the element type by
  search. A single in-scope witness is used; multiple distinct witnesses are an
  ambiguity error, governed by property-tested coherence
  ([equivalence-checking.md](../../roadmap/predicate_typing/equivalence-checking.md)):
  candidates that agree resolve silently, candidates that disagree produce a
  counterexample error.

## Type arguments versus module arguments

A functor's **type** argument is read off the call's carried argument type
(`List(Number)` yields `Number`), a projection rather than a search — list
literals and other parameterized values carry their type arguments
([ktype/parameterization-and-variance.md § Runtime type-parameter carriers](ktype/parameterization-and-variance.md#runtime-type-parameter-carriers)).
A functor's **module** (witness) argument is found by implicit search.

The rule: type arguments come from carried types; module arguments come from
search. An implicit functor parameterized over `:Type` therefore does not take an
implicit parameter, and does not conflict with the restriction that implicit
modules cannot take implicit parameters.

Dependent parameters — a value parameter whose type is an earlier type parameter
— are expressed by lifting the type to a parameter of the outer FN
(`FN (MAKEIT Ty :Type) -> Module = (MODULE built = ((FN (MAKE elt :Ty) -> ...)))`),
with the type argument either
inferred from the value's carried type or named through a `USING-SCOPE` block.

## Disambiguation

When several candidates resolve at one call site, a `USING-SCOPE` block names the
chosen module. This is the explicit form the implicit search elides; the same
block selects between distinct operation-bearing witnesses (an ascending versus a
descending `Ord`, say) that coherence checking reports as non-interchangeable.

## Open work

- [Stage 5 — Modular implicits](../../roadmap/predicate_typing/modular-implicits.md)
  — the resolver that selects and applies generic functors, including solving a
  functor's type argument from the call's carried argument types. Until it lands,
  generic functors are selected and applied by hand.
