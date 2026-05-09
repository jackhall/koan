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

**Directions.**

- *Engine location — decided.* Rust-side, structured as a self-contained
  subsystem separate from the dispatcher and scheduler. Two reasons for
  the separation: the engine is reusable as a general testing tool, and
  keeping it out of dispatch keeps the dispatcher and scheduler simple.
  The engine sees quoted axioms and the module's `gen` slot — invocation
  at ascription is the integration point, not a coupling at the
  implementation level.
- *Axiom syntax — decided per [design/module-system.md § Axioms and property testing](../design/module-system.md#axioms-and-property-testing).*
  `(AXIOM #(quoted bool predicate))` inside a `SIG` body. The engine
  evaluates each quote under a scope it builds by drawing samples from the
  module's `gen` slot for every free identifier; variable types resolve
  through the surrounding signature scope. The `IMPLIES` combinator
  handles conditional axioms via discard.
- *Generators live in modules — decided.* A `LET gen = (FN ...)` slot in a
  signature body is a structural obligation. Every ascribing module must
  supply a generator for the abstract type. No sidecar generator registry.
  Generators compose through functor application: a functor body builds
  the result module's `gen` from its parameter's `gen`.
- *Built-in type generators — decided.* The engine ships `Random`-using
  generators for `Number`, `Str`, `Bool`, `List<T>`, `Dict<K, V>`, etc. —
  these are the leaves of the composition story.
- *Missing-generator policy — decided.* The structural-conformance check
  at ascription rejects modules without a `gen` slot; nothing to skip
  silently.
- *Counterexample shrinking — deferred.* Adopt whatever shrinking
  algorithm Python's [Hypothesis](https://hypothesis.readthedocs.io/)
  library uses; that library's approach is the reference design. Pick the
  exact algorithm after investigating Hypothesis's implementation.
- *Sample size and budget — decided.* Sample count scales with the
  generator's type complexity, capped at 100. `Bool` exhausts at 2;
  `Number` needs a handful; `List<T>` needs more than `T`; `List<List<T>>`
  more again. The engine derives the count from the generator structure
  rather than taking a single global config knob.

## Dependencies

**Requires:**
- [Generalize `Scope::out` into monadic side-effect capture](monadic-side-effects.md)
  — generators thread randomness via the `Random` effect module rather than
  ambient entropy.

**Unblocks:**
- [Stage 6 — Equivalence-checked coherence](module-system-6-equivalence-checking.md)

The engine is independent of implicit dispatch and could be developed in
parallel with stage 5 — its integration point is the module language's
ascription site, which is already in place.
