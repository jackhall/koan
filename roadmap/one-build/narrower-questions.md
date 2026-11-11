# Narrower questions

**Optional.** No program yet counts the questions a witnessed layer records
or the units it holds.

**Problem.** A witnessed layer admits a binding by the answered questions its
unit records hold, and holds every unit a door handing back a type reached
([resumption](../../src/build/README.md#resumption)). Its admission lists each
question apart: two order questions on one name are two conditions, and a
question on a name nested in a composite is asked by substitution into that
type at every admission. In the first walk, a read that finds a module
binder, a path or a class by content holds its unit, hit or miss, and so does
a `never_satisfies` that decides *never*, since its refusal's wording reads an
end ([`judge.rs`](../../src/elaborate/judge.rs)).

**Acceptance criteria.**

- The bare order questions a witnessed layer records on one name are one
  interval condition, and its disjointness questions on one name one set, and
  the layer admits exactly the bindings the separate questions admit.
- A door answers, for a type naming a lexical variable and a concrete type,
  the interval of bindings under which the relation holds, exact where
  reproducible and a safe subset otherwise, and a witnessed layer records a
  nested positive question as the bare one that interval gives.
- A read by content holds its unit only where a unit the first walk stood
  fixes a module binder that can hold a module.
- A refusal's wording is rendered through the binding's table of closed
  types, so a `never_satisfies` that decides *never* leaves its unit shared.
- The commutation law and the witnessed laws in the resumption suite
  ([properties](../../src/build/README.md#properties)) hold, and every program
  prints as before.

**Directions.**

- *Compression — deferred* until a program's question lists cost: `join` and
  `meet` are exact
  ([the laws](../../lattice/src/types/laws.md#join-and-meet-are-exact)), so
  nothing is admitted that the questions do not admit.
- *The lattice door — deferred* until a program counts: the collector run
  with the variable's level as a quantified position
  ([the unifier](../../lattice/src/types/solving.md#the-unifier-collects-it-does-not-bind)),
  changing nothing at admission. It covers no nested negative, and it is a
  lattice change.
- *Reads by content — deferred* until a program counts: whether a binder can
  hold a module is read off its declared type, which no binding changes.
- *A refusal's wording — deferred* until a program counts.

## Dependencies

**Requires:** none — its substrate has shipped.

**Unblocks:** none — a leaf.
