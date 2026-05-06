# Module system stage 2 — Functors

**Problem.** Stage 1 lands structures and signatures with abstract types, but
generic data structures still need parameterization. A `Set` should be one
abstraction usable for any element type with an ordering — written once,
instantiated many times. Without **functors** (modules parameterized by other
modules) every concrete `Set<T>` would be hand-written.

**Impact.**

- *Generic data structures.* `MakeSet(Element)` and `MakeMap(Key)(Value)`
  write a data structure once and instantiate it against any element type
  with the required operations — no per-element-type duplication of
  `IntSet`, `StringSet`, `PointSet`.
- *Standard library gets its natural shape.* Collections, ordered maps,
  hash tables, and other parameterized abstractions ship in their canonical
  `MakeX(Element)` form rather than as per-concrete-type duplicates or
  builtins.
- *Substrate for stage 5.* Modular implicits resolve to functor
  applications — the compiler chooses `MakeSet(IntOrd)` when inferring a
  `Set<Number>`. Stage 5 has something meaningful to dispatch on rather
  than reducing to "pick a hand-written module."

**Directions.** None decided.

- *Functor declaration syntax.* Following stage 1's choice of module/signature
  surface form. The functor takes one or more named module arguments and
  returns a structure ascribed to a signature.
- *Sharing constraints.* `with type elt = E.t` lets a functor's output
  signature refine its abstract type to match the input. Mechanically a
  constraint on the output's signature; needs to thread through the type
  checker.
- *Generative vs applicative semantics.* Generative — each application
  produces a fresh abstract type — is simpler to specify and provides the
  per-type identity property the design relies on. Applicative — same
  arguments yield the same output type — is more ergonomic when functors are
  re-applied. Recommended: generative for v1, revisit later. The decision
  lives here.
- *Multi-argument functors.* `MakeMap(Key)(Value)`. Curried application is the
  natural form; concrete syntax follows stage 1's conventions.
- *Type identity through functor application.* `MakeSet(IntOrd)` applied
  twice yields two distinct `Set` types. The implementation extends stage 1's
  module-type identity carrier to include the application context.

## Dependencies

**Requires:**
- [Stage 1 — Module language](module-system-1-module-language.md)

**Unblocks:**
- [Stage 3 — First-class modules](module-system-3-first-class-modules.md)
