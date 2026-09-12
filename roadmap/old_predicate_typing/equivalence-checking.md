# Module system stage 6 — Equivalence-checked coherence

**Problem.** Stage 5's automatic resolution makes coherence the language's
problem rather than the user's. Before stage 5, witnesses are passed by
hand and the user owns which dictionary applies where; once
`SEARCH_IMPLICIT` picks witnesses from scope, two different witnesses for
the same signature can be selected at different call sites and produce
silent wrong answers when values built under one ordering get queried
under another. Stage 5's strict-on-ambiguity policy errors when multiple
implicits could resolve a search, which is correct when the
implementations are genuinely different but a tax when they're observably
interchangeable. Stage 6 is the **coherence layer for stage 5's
resolution**: it **tests candidates against each other** with the
property-testing engine — agreement means silent ambiguity is safe, and
disagreement becomes a counterexample-bearing error.

This is the differentiating coherence story. Strict trait systems (Rust,
Haskell) prevent it via global orphan rules; lax ones (Scala) silently pick.
Property-tested coherence is a third option.

**Acceptance criteria.**

- When two `Ord` candidates agree under property testing on every sampled
  input, the resolver picks one silently rather than raising stage 5's
  ambiguity error.
- When two candidate implicits for the same signature disagree on a sampled
  input, the resolver raises a counterexample-bearing error naming the
  disagreeing inputs and outputs.
- Two distinct `Hash` implementations resolved for the same signature
  disagree under property testing and produce that counterexample-bearing
  error.

**Directions.**

- *Detection of multi-candidate search — decided.* Stage 5's resolver
  already enumerates candidates; stage 6 adds the path that runs them
  through equivalence testing before committing.
- *Sample size for equivalence — deferred.* Reuses stage 4's
  complexity-scaled count; the cap may differ since equivalence testing's
  cost amortizes across every call site that resolves to the same
  candidate set. Pick after stage 4's scaling has been observed in
  practice.
- *Observation declarations — decided in concept, syntax open.* A
  signature can specify a coarser observation than direct value equality
  (`observation compare via sign` or similar). Needed for signatures whose
  return values are deliberately under-specified. Concrete syntax to be
  picked.
- *Caching — decided.* Pairwise equivalence between two implicits depends
  only on the candidates' definitions, not on the call site. Cache by
  (signature, candidate-pair) to avoid re-running tests on every compile.
- *Error message shape — decided.* The counterexample bearing the
  disagreement is the payoff feature — the user sees concrete inputs and
  outputs and immediately understands why the candidates aren't
  interchangeable. Worth disproportionate engineering investment.

## Dependencies

**Requires:**

- [Stage 4 — Property testing and axioms](axioms-and-generators.md)
- [Stage 5 — Modular implicits](modular-implicits.md)

**Unblocks:** none — stage 6 is a leaf.
