# Module system stage 2 — Functors

**Problem.** Structures and signatures with abstract types are in the
language (stage 1), but generic data structures still need parameterization.
A `Set` should be one abstraction usable for any element type with an
ordering — written once, instantiated many times. Without **functors**
(modules parameterized by other modules) every concrete `Set<T>` would be
hand-written.

**Impact.**

- *Generic data structures.* `(MAKESET Element)` and curried
  `((MAKEMAP Key) Value)` write a data structure once and instantiate it
  against any element type with the required operations — no
  per-element-type duplication of `IntSet`, `StringSet`, `PointSet`.
- *Standard library gets its natural shape.* Collections, ordered maps,
  hash tables, and other parameterized abstractions ship as functor FNs
  rather than as per-concrete-type duplicates or builtins.
- *Substrate for stage 5.* Modular implicits resolve to functor
  applications — the compiler chooses `(MAKESET IntOrd)` when inferring a
  `Set<Elt: Number>`. Stage 5 has something meaningful to dispatch on
  rather than reducing to "pick a hand-written module."

**Directions.** Surface syntax decided in
[design/module-system.md](../design/module-system.md#functors); implementation
choices below.

- *Functor declaration syntax — decided.* Functors are FNs whose parameters
  are signature-typed and whose body returns a `MODULE` expression. No
  `FUNCTOR` keyword.
- *Sharing constraints — decided.* Pinning a functor's output abstract type
  to its input rides on named-slot syntax for parameterized type expressions
  (`<Type: E.Type>`), not a separate `with type` keyword. See
  [design/module-system.md](../design/module-system.md#parameterized-type-expressions).
- *Generative vs applicative semantics.* Generative — each application
  produces a fresh abstract type — is simpler to specify and provides the
  per-type identity property the design relies on, and falls out of
  `:|`-per-call evaluation. Applicative — same arguments yield the same
  output type — is more ergonomic when functors are re-applied. Recommended:
  generative for v1, revisit later. The decision lives here.
- *Multi-argument functors.* Ordinary multi-parameter FNs; currying is just
  nested FNs. No special multi-application form.
- *Type identity through functor application.* `(MAKESET IntOrd)` applied
  twice yields two distinct `Set` types. The implementation extends stage 1's
  module-type identity carrier to include the application context.
- *Higher-kinded abstract type slots.* Signatures need to declare type
  constructors (a `Wrap` slot taking a type parameter) so monads and other
  parametric abstractions are expressible. Required by
  [monadic-side-effects](monadic-side-effects.md).

## Dependencies

**Requires:**
- [Stage 1.5 — Scheduler integration](module-system-1.5-scheduler.md) —
  sharing constraints and generative-functor type identity ride on the
  type-checker substrate stage 1.5 lands.

**Unblocks:**
- [Error-handling surface follow-ups](error-handling.md) — `Result<T, E>`
  is the functor-produced carrier for user-typed errors.
- [Generalize `Scope::out` into monadic side-effect capture](monadic-side-effects.md)
  — the in-language `Monad` signature's `Wrap` slot is higher-kinded,
  expressible only with functor support.
