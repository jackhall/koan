# Module system stage 7 — Syntax tuning and witness types

**Problem.** Stage 5 ships with explicit module application as the only
disambiguation form, deliberately ugly so it does not accidentally become
the final answer. Stage 7 collects the patterns that emerge from real
stage-5 code and designs proper sugar against them. It also lands the
**witness types** escape hatch: an opt-in mechanism for users who want
stronger-than-probabilistic coherence at the cost of more verbose types.

This is the polish stage; everything below is additive over stages 1-6 and
breaks no existing programs.

**Impact.**

- *Disambiguation is verbose.* Stage 5 leaves explicit-application syntax
  as the lowest-level form. Routine disambiguation in real code wants
  block-scoped binding, module-level priority, or selective imports.
  Without sugar this is a tax on the cases where coherence checking can't
  silently pick.
- *Property-tested coherence has limits.* It is probabilistic; an
  adversarial pair of modules that agree on the test distribution but
  disagree elsewhere passes. Some users want deductive certainty.
  Witness types deliver it by reflecting the implicit's identity in the
  abstract type — distinct phantoms produce distinct types, and the type
  system enforces non-mixing.

**Directions.** None decided.

- *Surveyed sugar candidates.* Block-scoped binding (apply a chosen
  implicit to many calls in a region), module-level priority (one module
  declared canonical, others warn), selective imports (`use mylib::sort
  except IntOrdReverse`). Pick from observed patterns from real stage-5
  code, not from imagination.
- *Explicit-application syntax.* Stage 5 ships a placeholder; stage 7
  fixes the form against the patterns of use. May coincide with the
  block-scoped binding form (block applies to many calls; explicit applies
  to one).
- *Witness type encoding.* Reflect the implicit's identity as a phantom
  type parameter on the abstract type — `Set<Number>` becomes
  `Set<Number, IntOrd>` when `IntOrd` is the implicit used. Distinct
  phantoms means distinct types means cannot mix. Type inference must
  elide the phantom in source so users only write it when they want.
- *Opt-in mechanism for witness types.* Witness types are not the default.
  A signature marks itself as participating (concrete syntax tbd) and
  consumers of that signature carry the phantom. The decision is
  per-signature, not per-use.

## Dependencies

**Requires:**
- [Stage 5 — Modular implicits](module-system-5-modular-implicits.md)

**Unblocks:** none — stage 7 is a leaf.

The deferred-syntax discipline relies on stage 5 having shipped with a
deliberately ugly placeholder; designing this stage before stage 5 has
real-world use defeats the purpose.
