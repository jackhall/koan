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

**Directions.** None decided.

- *Initial collection set.* Which data structures ship in v1? `Set`,
  `Map`, `List` extensions are obvious; `OrderedMap`, `HashMap`,
  `Sequence` are candidates. Ship the minimum that exercises the functor
  patterns the language depends on, defer the rest.
- *Layout under the stdlib root.* Flat (`std/Set.koan`,
  `std/Map.koan`) or grouped (`std/collections/Set.koan`)? Falls out
  of the [files-and-imports](files-and-imports.md) layout decision.
- *Naming conventions for ordering / equality / hashing modules.*
  `IntOrd`, `StringOrd`, `IntEq` — the canonical implicit modules users
  pass to functor applications. The shapes are constrained by the
  signature names in the stdlib's collection functors; the surface form
  needs a deliberate convention.
- *Builtin retirement.* Where the stdlib supersedes a Rust-side builtin
  (e.g., the existing dictionary builtin if Set/Map cover its uses), the
  builtin gets removed in the same change rather than left as a parallel
  surface.

## Dependencies

**Requires:**
- [Module system stage 2 — Module values and functors through the scheduler](module-system-2-scheduler.md)
  — collections are functor FNs over their element/key types, which need
  end-to-end functor definition, dispatch, and execution.
- [Files and imports](files-and-imports.md) — the stdlib lives across
  multiple `.koan` files, so user code needs a way to load them.

**Unblocks:**
- [Generalize `Scope::out` into monadic side-effect capture](monadic-side-effects.md)
  — the standard effect modules (`Random`, `IO`, `Time`) ship as stdlib
  entries.
