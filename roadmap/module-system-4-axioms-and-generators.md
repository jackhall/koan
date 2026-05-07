# Module system stage 4 — Property testing and axioms

**Problem.** Signatures from stages 1-2 declare *shapes* — names and types of
operations. They don't declare *meaning*. A `compare` function that isn't
transitive is a bug, but the type system has no way to catch it. Stage 4 adds
**axiom** syntax to signatures and a **property-testing engine** that runs
those axioms against random samples when a structure ascribes the signature.
Failures are ascription errors with reported counterexamples.

The engine doubles as a general testing tool for ordinary Koan code, useful
independent of signatures.

**Impact.**

- *Invalid implementations are caught at the ascription site.* A
  non-transitive `compare`, a hash that disagrees with its own equality, a
  monoid whose identity isn't — common bug shapes that no other mechanism
  catches surface as ascription errors with reported counterexamples instead
  of silent runtime wrongness.
- *Mechanical contract checking.* Signature documentation that says "this
  must be transitive" stops being convention-only; the compiler verifies
  the property at the ascription site.
- *Substrate for stage 6.* Cross-implicit equivalence checking reuses the
  same engine with a different axiom shape; stage 6 has something to call
  into.
- *General testing tool, beyond signatures.* The same engine doubles as a
  property-testing tool for ordinary Koan code, useful even where no
  signature is involved.

**Directions.** None decided.

- *Engine location.* Rust-side, structured as a self-contained subsystem
  separate from the dispatcher and scheduler. Two reasons for the separation:
  the engine is reusable as a general testing tool, and keeping it out of
  dispatch keeps the dispatcher and scheduler simple. The engine sees axioms
  and types but not modules per se — invocation at ascription is the
  integration point, not a coupling at the implementation level.
- *Axiom syntax in signatures.* `axiom name : forall x. property` is the
  shape; concrete syntax follows stage 1's keyword conventions.
- *Generators are not part of signatures.* The design deliberately keeps
  generators out of the module language. The engine ships generators for
  built-ins (`Number`, `Str`, `Bool`, `List<T>`, `Dict<K, V>`, etc.) by
  composition; user-type generators are registered alongside the type via the
  engine's public surface, not via signature declarations.
- *Missing-generator policy.* When a signature axiom quantifies over a type
  with no available generator, axiom checking is skipped with a diagnostic.
  Whether this is a warning (default) or an error (opt-in stricter mode) is a
  design dial.
- *Counterexample shrinking.* The engine should shrink to a minimal failing
  case where the generator infrastructure permits. Standard
  property-testing technique; pick a shrinking algorithm.
- *Sample size and budget.* How many samples per axiom? Configurable
  per-build with a sensible default. Compile-time cost is real — needs a
  budget so a signature with many axioms doesn't dominate compilation.

## Dependencies

**Requires:**

**Unblocks:**
- [Stage 6 — Equivalence-checked coherence](module-system-6-equivalence-checking.md)

The engine is independent of implicit dispatch and could be developed in
parallel with stages 2-3 — its integration point is the module language's
ascription site, which is already in place.
