# Module system stage 4 — Property testing and axioms

**Problem.** Signatures from stages 1-2 declare *shapes* — names and types of
operations. They don't declare *meaning*. A `compare` function that isn't
transitive is a bug, but the type system has no way to catch it. Stage 4 adds
**axiom** syntax to signatures and a **property-testing engine** that runs
those axioms against random samples when a structure ascribes the signature.
Failures are ascription errors with reported counterexamples.

The engine doubles as a general testing tool for ordinary Koan code, useful
independent of signatures.

**Acceptance criteria.**

- A module ascribing a signature whose `compare` is non-transitive (or whose
  hash disagrees with its own equality, or whose monoid identity isn't) is
  rejected at the ascription site with a reported counterexample.
- An `(AXIOM ...)` declared in a `SIG` body runs against generated samples
  when a structure ascribes that signature, and a violating sample fails the
  ascription.
- The property-testing engine runs against ordinary Koan code outside any
  signature, reporting a counterexample when a predicate fails on a generated
  sample.

**Directions.**

- *Engine location — decided.* Rust-side, structured as a self-contained
  subsystem separate from the dispatcher and scheduler. Two reasons for
  the separation: the engine is reusable as a general testing tool, and
  keeping it out of dispatch keeps the dispatcher and scheduler simple.
  The engine sees quoted axioms and the module's `gen` slot — invocation
  at ascription is the integration point, not a coupling at the
  implementation level.
- *Axiom syntax — decided per [design/typing/implicits.md § Axioms and property testing](../../design/typing/implicits.md#axioms-and-property-testing).*
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
  generators for `Number`, `Str`, `Bool`, `:(LIST OF Ty)`, `:(MAP Ke -> Va)`,
  etc. —
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
  `Number` needs a handful; `:(LIST OF Ty)` needs more than `Ty`;
  `:(LIST OF :(LIST OF Ty))`
  more again. The engine derives the count from the generator structure
  rather than taking a single global config knob.

## Dependencies

The engine is independent of implicit dispatch and could be developed in
parallel with stage 5 — its integration point is the module language's
ascription site, which is already in place.

**Requires:**

- [Generalize `Scope::out` into monadic side-effect capture](../libraries/monadic-side-effects.md)
  — generators thread randomness via the `Random` effect module rather than
  ambient entropy.

**Unblocks:**

- [Stage 6 — Equivalence-checked coherence](equivalence-checking.md)
