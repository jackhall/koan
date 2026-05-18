# Modular implicits

A function can declare an **implicit module parameter**:

```
LET sort = (FN (SORT xs :(List Mo.Type) {Mo: OrderedSig}) -> :(List Mo.Type) = (...))
```

At a call site `(SORT [3, 1, 2])`, the compiler infers `Mo.Type = Number`,
searches in scope for a module satisfying
`(SIG_WITH OrderedSig ((Type: Number)))`, and inserts it. Searching is
**lexical**: the candidate set is the implicit modules defined in the current
module plus those explicitly imported. Nothing leaks through transitive
dependencies.

Specificity is the standard rule: more-specific candidates beat
less-specific ones; ties between unrelated candidates are an error. The
disambiguation primitive when ambiguity arises is **explicit module
application** at the call site — the user names which candidate to use.

Two surface forms are open: the implicit-parameter marker (`{...}` is a
placeholder above) and the **explicit-application form** — the lowest-level
surface for naming a candidate at the call site to break an ambiguity.
Examples in this file and in the
[equivalence-checking section](#cross-implicit-equivalence-checking) that
read OCaml-shaped (`with type t = ...`, `module type ... = sig ... end`,
`observation ... via ...`) are pre-Koan placeholders. The surface forms,
along with sugar (block-scoped binding, module priority, selective imports),
are tracked in [open-work.md](open-work.md). The constraint half — the
signature expression to the right of `:` — uses the `SIG_WITH` builtin
described in
[functors.md § Type expressions and constraints](functors.md#type-expressions-and-constraints).

## Higher-order restriction

**Implicit modules cannot themselves take implicit parameters.** A normal
functor can take implicit module arguments at its application site, but the
*resolution* never recurses through implicit search to satisfy an implicit
parameter of a candidate module. This cuts the worst tangle in the
modular-implicits design space — search staying one level deep keeps both
inference and resolution decidable in the simple case, and avoids the
exploding-search-tree pathology that has historically blocked OCaml's
modular-implicits proposal.

## Axioms and property testing

A signature can carry **axioms** — propositional contracts on its
operations:

```
SIG OrderedSig = (
  (LET Type = ...)
  (VAL compare :(Function (Type, Type) -> Number))
  (VAL gen :(Function (Random) -> Type))

  (AXIOM #((compare x x) = 0))
  (AXIOM #((sign (compare x y)) = (- (sign (compare y x)))))
  (AXIOM #(IMPLIES (and (<= (compare x y) 0) (<= (compare y z) 0))
                   (<= (compare x z) 0)))
)
```

Each `AXIOM` carries a quoted bool expression — the `#(...)` sigil produces
a `KExpression` value. At ascription time, the engine evaluates each
axiom's quote under a scope it builds by drawing samples from the module's
`gen` slot for every free identifier the quote references. Variable types
are resolved through the surrounding signature scope: `x`, `y`, `z` above
take type `Type` because `compare`'s parameters fix that kind.

`IMPLIES` is the engine's discard combinator — when the antecedent is
false, the sample is dropped without counting against the test budget,
matching standard property-based-testing practice for conditional axioms.

When a structure ascribes a signature with axioms, the engine runs each
axiom against random samples. Failures are ascription errors with a
reported counterexample (and shrunk to a minimal case where the engine
permits). This catches *invalid* implementations mechanically —
non-transitive comparisons, hashes that disagree with their own equality,
monoids whose identity isn't.

**Generators live in modules; the signature requires them.** A
`(VAL gen :(Function (Random) -> Type))` slot in a signature body is an
obligation: every ascribing module must supply a generator for the abstract
type. This folds
generator presence into the existing structural-conformance check —
ascription of a module without a `gen` slot fails with the same
"missing field" error as any other unsupplied operation. There is no
sidecar generator registry.

**Generators compose through functor application.** A functor body that
constructs the result module's `gen` from the parameter's `gen` —
`MakeSet(Er)`'s `gen` builds set samples by drawing from `Er.gen` — gives
the composed module its generator mechanically. Composition is a
module-language property; the engine just calls `Mo.gen` like any other
operation.

The **property-testing engine** is a Rust-side subsystem of the compiler,
deliberately disjoint from the dispatcher and the scheduler. Two reasons
for the separation: (a) the engine is reusable as a general testing tool
against ordinary Koan code, not only against signature axioms; (b) keeping
it out of the dispatcher and scheduler keeps both simple. The engine's job
is to take a quoted axiom, generate samples for the variables it binds via
the module's `gen`, evaluate the quote, and report counterexamples;
nothing in the engine knows about modules or implicits per se.

**Randomness threads as a monadic effect.** Generators take a `Random`
parameter rather than consuming raw entropy ambiently — see
[design/effects.md](../effects.md). The property-testing engine threads
the random module through generator calls explicitly until modular implicits
land, which is why the axioms stage depends on the in-language monadic-effect
surface.

## Cross-implicit equivalence checking

When implicit search finds multiple candidates that all satisfy the same
query, the compiler runs a **behavioral equivalence test** between them
using the property-testing engine:

```
For every pair (M, N) of in-scope candidates for ORDERED with type t = T:
  for sampled x, y from T's generator:
    assert M.compare(x, y) == N.compare(x, y)
```

If candidates agree on all sampled inputs, ambiguity is silent — pick either,
they're observably the same. If they disagree, the search fails with a
counterexample-bearing error:

```
error: ambiguous implicit ORDERED with type t = Number — and the candidates disagree
  IntOrd.compare(5, 3) = -1
  IntOrdReverse.compare(5, 3) = +1
  these modules are not behaviorally equivalent; pick one explicitly
```

This is the **coherence story**. It is probabilistic, not a proof — a
sufficiently adversarial pair of modules that agree on the sampled
distribution but disagree elsewhere will pass. For the common bug shapes
(reversed orderings, off-by-one comparisons, different hash seeds) the
disagreement is dense and the test catches it on the first sample. For
signatures where the operation deliberately under-specifies the return value
(`compare` where only the *sign* matters), a signature can declare an
**observation** that the equivalence check uses instead of direct value
equality:

```
module type ORDERED = sig
  ...
  observation compare via sign
end
```

When stronger guarantees are required, **witness types** (an opt-in feature
in the syntax-tuning stage) reflect the implicit's identity in the abstract
type itself — `Set<Number, IntOrd>` and `Set<Number, IntOrdReverse>` become
distinct types that cannot mix. Ergonomic but verbose; a tool for the cases
where probabilistic coherence isn't enough.

The OCaml-shaped fragments in this section (the `with type t = ...`
ambiguity-error message and the `module type ORDERED = sig ... observation
compare via sign end` observation declaration) are placeholders. The
explicit-disambiguation form, the ambiguity-error rendering, and the
observation-declaration syntax are tracked in [open-work.md](open-work.md).

## Resolution and coherence: the design dials

| Dial | Setting |
| --- | --- |
| Implicit search scope | Lexical + explicitly imported only. No transitive leak. |
| Specificity | Most-specific-wins. Unrelated ties are errors. |
| Ambiguity policy | Strict — error unless candidates pass cross-equivalence. |
| Coherence | By convention, with property-tested equivalence as a safety net. Witness types opt-in for stronger guarantees. |
| Orphan rule | Soft (lint-only). Implicits should live with their signature or with one of their dispatched types; deviating warns but doesn't error. |
| Higher-order implicits | Disallowed. Implicit modules cannot take implicit parameters. |
| Disambiguation primitive | Explicit module application; surface syntax open ([open-work.md](open-work.md)). |
