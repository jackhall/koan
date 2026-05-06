# Trait inheritance

**Problem.** A trait that requires another trait — `Ord` extending `Eq`, `Iterator`
extending `IntoIterator` — is the standard way to layer abstractions. Without it, every
"richer" trait has to redeclare every operation of every "base" trait it conceptually
depends on, and the dispatcher cannot infer that any `Ord` value also satisfies `Eq`.

**Impact.**

- *Layered trait hierarchies.* `Ord EXTENDS Eq` (or the colon-form analog) declares the
  relationship once; `FN sort (xs: List<T>) WHERE T: Ord` no longer has to re-state the
  `Eq` requirement at every use site.
- *Dispatch follows the chain.* A function that takes an `Eq` value also accepts an `Ord`
  value — the dispatcher walks the inheritance chain to satisfy the predicate rather
  than treating each trait as a wholly separate carrier.

**Directions.** None decided.

- *Declaration syntax.* `TRAIT Ord EXTENDS Eq = (...)` or `TRAIT Ord: Eq = (...)`. The
  keyword form reads better in Koan's existing keyword-heavy surface; the colon form is
  shorter and matches the type-annotation syntax elsewhere.
- *Implementation obligation.* Implementing `Ord` should either require an `Eq` impl to
  exist (checked when the impl registers) or assume one exists at dispatch time and
  error if not (checked lazily on the first call that needs it). Eager checking is
  friendlier; lazy is cheaper.
- *Diamond inheritance.* If `A` extends `B` and `C`, and both `B` and `C` extend `D`, the
  dispatcher has to handle the shared base. Single-inheritance chains avoid the question;
  multi-inheritance demands a resolution rule (linearization, or disallow conflicting
  requirements outright).
- *Dispatch priority within an inheritance chain.* Already half-decided by the priority
  rule in the previous entry: a more-derived trait beats a less-derived one (`concrete >
  Ord > Eq > Any`).

## Dependencies

**Requires:**
- [`TRAIT` builtin for structural typing](traits.md) — sets up single traits and the
  priority rule; this entry extends both.

**Unblocks:**
- [Static type checking and JIT compilation](static-typing-and-jit.md)

Last of the type/trait sequence. Punting it doesn't block anything else — trait
inheritance is purely additive over the trait substrate.
