# Module system stage 6 — Equivalence-checked coherence

**Problem.** Stage 5's strict-on-ambiguity policy errors when multiple
implicits could resolve a search. Sometimes that's correct: two genuinely
different implementations need the user to disambiguate. Sometimes it's
overconservative: two implementations that produce the same observable
behavior are interchangeable, and erroring on their coexistence is a tax.
Stage 6 closes this gap by **testing the candidates against each other**
with the property-testing engine — agreement means silent ambiguity is
safe, and disagreement means the user gets a counterexample-bearing error.

This is the differentiating coherence story. Strict trait systems (Rust,
Haskell) prevent it via global orphan rules; lax ones (Scala) silently pick.
Property-tested coherence is a third option.

**Impact.**

- *Stage 5 over-errors.* Two `Ord` instances that agree on every input still
  trigger ambiguity errors. The user has to manually disambiguate even when
  the choice doesn't matter.
- *No protection against silent inconsistency in scoped designs.* Without
  global orphan rules, a program can have two scopes each pick a different
  but valid implicit, and values built under one ordering get queried under
  another — silent wrong answers. Property-tested equivalence is the
  mechanical safety net.
- *Hash-style operations.* Two `Hash` implementations always disagree on
  most inputs (different hash functions are different functions). Property
  testing flags this immediately; the design treats it as a feature —
  disagreement is the signal that mixing the two breaks `HashMap`
  correctness, regardless of whether each implementation is individually
  valid.

**Directions.** None decided.

- *Detection of multi-candidate search.* Stage 5's resolver already
  enumerates candidates; stage 6 adds the path that runs them through
  equivalence testing before committing.
- *Sample size for equivalence.* Same configurability as stage 4's axiom
  testing. Likely a higher default than per-axiom, since the cost amortizes
  across all call sites that resolve to the same candidate set.
- *Observation declarations.* `observation compare via sign` (or similar)
  lets a signature specify a coarser observation than direct value equality.
  Needed for signatures whose return values are deliberately
  under-specified. Concrete syntax to be picked.
- *Caching.* Pairwise equivalence between two implicits depends only on the
  candidates' definitions, not on the call site. Cache by (signature,
  candidate-pair) to avoid re-running tests on every compile.
- *Error message shape.* The counterexample bearing the disagreement is the
  payoff feature — the user sees concrete inputs and outputs and
  immediately understands why the candidates aren't interchangeable. Worth
  disproportionate engineering investment.

## Dependencies

**Requires:**
- [Stage 4 — Property testing and axioms](module-system-4-axioms-and-generators.md)
- [Stage 5 — Modular implicits](module-system-5-modular-implicits.md)

**Unblocks:** none — stage 6 is a leaf.
