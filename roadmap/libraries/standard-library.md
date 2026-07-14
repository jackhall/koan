# Standard library

**Problem.** Koan has no standard library written in Koan itself.
Everyday data structures — sets, ordered maps, hash tables, lists with
the operations the user expects — are either absent or hand-wired as Rust
builtins, and once functors and a multi-file source surface are in place
there is no canonical body of Koan code that demonstrates how to *use*
them. Users learn the language without a worked example of idiomatic
module / signature / functor / import composition because that example
doesn't exist yet. The builtin `Result` type also ships with no
combinators, so user code chaining fallible computations writes MATCH
boilerplate by hand for every step.

**Acceptance criteria.**

- `Set`, `Map`, `OrderedMap`, and friends are written once as Koan-source
  functors over the required operations (`Element` with an ordering, `Key`
  with equality, etc.) and instantiated by user code at multiple element
  types from that single definition.
- The stdlib is a directory of `.koan` files that user code imports through
  the same surface every Koan project uses.
- The standard effect modules (`Random`, `IO`, `Time` from
  [monadic-side-effects](monadic-side-effects.md)) ship as stdlib entries
  ascribing the in-language `Monad` signature.
- A reader points to a directory of stdlib Koan code that exercises modules,
  signatures, functors, imports, and effects together as a worked example.
- `map`, `bind`, `unwrap_or`, and friends over the builtin `Result` type let
  user code thread a value through fallible steps without a hand-written
  MATCH at each one.

**Directions.**

- *Initial collection set — open.* Which data structures ship in v1? `Set`,
  `Map`, `List` extensions are obvious; `OrderedMap`, `HashMap`,
  `Sequence` are candidates. Recommended: ship the minimum that exercises
  the functor patterns the language depends on, defer the rest.
- *Layout under the stdlib root — deferred.* Pending further design in
  [files-and-imports](files-and-imports.md) — the open file-to-module
  mapping and qualification-after-import choices there shape what a
  natural stdlib layout looks like.
- *Naming conventions for ordering / equality / hashing modules — open.*
  `IntOrd`, `StringOrd`, `IntEq` — the canonical implicit modules users
  pass to functor applications. The shapes are constrained by the
  signature names in the stdlib's collection functors; the surface form
  needs a deliberate convention.
- *`Result` combinator set — open.* `map`, `bind`, `unwrap_or`, and
  friends are Koan source over the builtin `Result` type, not Rust
  builtins. The `Result` constructor itself shipped (see
  [design/error-handling.md](../../design/error-handling.md)), so user
  code can construct and MATCH on `Result` before these land. Which
  combinators ship in v1 is open.
- *Builtin retirement — decided.* Where the stdlib supersedes a Rust-side
  builtin (e.g., the existing dictionary builtin if Set/Map cover its
  uses), the builtin gets removed in the same change rather than left as
  a parallel surface.
- *Applicative functor semantics — deferred to predicate typing.* Stage
  5's implicit resolution makes independent `(MakeSet)` call sites
  resolve to the same `IntOrd` without users seeing it; under the
  generative-only semantics spec'd in
  [design/typing/functors.md](../../design/typing/functors.md), two
  such applications mint distinct Set types and the resulting sets
  cannot interoperate. Applicative semantics — same-functor-applied-to-
  same-module produces equal types — closes this. The decided seam is
  the `FUNCTOR` binder's `is_functor` flag; the memoization scheme
  (argument-identity hashing vs. structural equality on argument values)
  is the open piece, deferred until predicate typing lands.

Canonical signatures use the `VAL` declarator
([design/typing/modules.md § Structures and signatures](../../design/typing/modules.md#structures-and-signatures))
for value slots. An `ORDERED` signature reads

```
SIG Ordered = (
  (LET Type = Number)
  (VAL compare :(FN (Type, Type) -> Number))
)
```

and a `SET` functor's signature reads

```
SIG Set = (
  (LET Elt = Number)
  (VAL empty :Type)
  (VAL insert :(FN (Type, Elt) -> Type))
)
```

— operations declared against the SIG's abstract `Type` / `Elt` members
directly, without standing in an example value of an arbitrary
concrete type.

## Dependencies

**Requires:**

- [Files and imports](files-and-imports.md) — the stdlib lives across
  multiple `.koan` files, so user code needs a way to load them.

**Unblocks:**

- [Generalize `Scope::out` into monadic side-effect capture](monadic-side-effects.md)
  — the standard effect modules (`Random`, `IO`, `Time`) ship as stdlib
  entries.
