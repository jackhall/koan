# Standard library

**Problem.** Koan has no standard library written in Koan itself.
Everyday data structures — sets, ordered maps, hash tables, lists with
the operations the user expects — are either absent or hand-wired as Rust
builtins, and once functors and a multi-file source surface are in place
there is no canonical body of Koan code that demonstrates how to *use*
them. Users learn the language without a worked example of idiomatic
module / signature / functor / import composition because that example
doesn't exist yet.

**Impact.**

- *Collections ship as Koan-source functor FNs.* `Set`, `Map`,
  `OrderedMap`, and friends are written once as functors over the
  required operations (`Element` with an ordering, `Key` with equality,
  etc.) and instantiated by user code — no per-element-type duplication,
  no Rust-side builtin shim per concrete type.
- *Standard library lives across multiple files.* The stdlib is a
  directory of `.koan` files, imported by user code through the same
  surface every Koan project uses, so it doubles as the canonical
  example of file-and-import structure.
- *Effect modules have a canonical home.* Standard effect modules
  (`Random`, `IO`, `Time` from
  [monadic-side-effects](monadic-side-effects.md)) ship as stdlib
  entries rather than as ad-hoc top-level definitions, so the in-language
  `Monad` story has working stdlib examples to point at.
- *Idiomatic Koan has a worked example.* New users have a body of Koan
  code that exercises modules, signatures, functors, imports, and
  effects together — so the answer to "how do I structure a real Koan
  program?" is a directory they can read.

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
- *Builtin retirement — decided.* Where the stdlib supersedes a Rust-side
  builtin (e.g., the existing dictionary builtin if Set/Map cover its
  uses), the builtin gets removed in the same change rather than left as
  a parallel surface.
- *Applicative functor semantics via `FUNCTOR` binder — open.* Stage 5's
  implicit resolution makes independent `(MakeSet)` call sites resolve
  to the same `IntOrd` without users seeing it; under the shipped
  generative-only semantics (per
  [design/module-system.md § Functors](../design/module-system.md#functors)),
  two such applications mint distinct Set types and the resulting sets
  cannot interoperate. Applicative semantics — same-functor-applied-to-
  same-module produces equal types — closes this. Landing form: a new
  `FUNCTOR` binder reusing FN mechanics, distinguished from `FN` at the
  surface so the generative/applicative choice is visible at the
  declaration. The memoization scheme (argument-identity hashing vs.
  structural equality on argument values) is part of this item.

## Dependencies

**Requires:**

- [Functor parameters — Type-class names and templated return types](module-system-functor-params.md)
  — collection functors like `Make` over `ORDERED` need Type-class FN
  parameters and return-type expressions that reference them.
- [Files and imports](files-and-imports.md) — the stdlib lives across
  multiple `.koan` files, so user code needs a way to load them.

**Unblocks:**

- [Generalize `Scope::out` into monadic side-effect capture](monadic-side-effects.md)
  — the standard effect modules (`Random`, `IO`, `Time`) ship as stdlib
  entries.
