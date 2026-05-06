# Module system stage 5 — Modular implicits

**Problem.** Stages 1-3 give an explicit module language: every functor
application, every module-typed argument, every signature constraint is
written by hand. For everyday generic code this is verbose. Stage 5
introduces **implicit module parameters**: a function declares that it
requires some module satisfying a given signature, and at the call site the
compiler resolves which module to thread in by searching scope. This is the
ergonomic payoff of the design.

**Impact.**

- *Generic code is verbose.* Every generic function requires an explicit
  module argument. `MakeSet(IntOrd)` and `sort(IntOrd, xs)` clutter call
  sites that could be `MakeSet()` and `sort(xs)`.
- *Standard-library design suffers.* The natural form for `sort`, `min`,
  `intersect`, `==` is "function takes the dictionary of operations
  implicitly." Without modular implicits these stay verbose or become
  builtins.
- *No multi-parameter dispatch.* The earlier trait roadmap left
  binary-operator dispatch (`+`, `==`, `intersect`) as an unresolved
  partial-order tie. With modular implicits, a multi-type signature handles
  it natively — implicit search dispatches on all types simultaneously.

**Directions.** None decided.

- *Implicit-parameter declaration syntax.* The function signature needs a
  slot for implicit module parameters; surface form follows stage 1's
  conventions.
- *Explicit-application disambiguation syntax.* The lowest-level form for
  resolving ambiguity at a call site. Surface form is **deliberately
  deferred** — explicit application ships in this stage with a placeholder
  syntax, and stage 7 designs the user-facing form against patterns from
  real code. The placeholder is intentionally ugly so it doesn't accidentally
  become the final answer.
- *Resolution algorithm.* Lexical scope plus explicitly imported implicits;
  filter by signature unification; pick the most specific; ambiguity is an
  error. Specificity rule: most-specific-wins, with unrelated ties as
  errors. See [the design doc's resolution dials
  table](../design/module-system.md#resolution-and-coherence-the-design-dials).
- *Inference and search interleaving.* Modeled as `Infer` and
  `ImplicitSearch` scheduler node types per the design doc. Stage 5 is where
  the cross-language phase boundary stabilizes — inference produces type
  refinements that search consumes; search produces module choices that
  refine types other inference tasks are waiting on.
- *Higher-order restriction.* Implicit modules cannot themselves take
  implicit parameters. Decided up front; documented and enforced in this
  stage. This is the architectural simplification that keeps resolution
  decidable and search-tree size bounded.
- *Error message investment.* When ambiguity errors fire, they need to name
  the candidate modules with their import paths and suggest the explicit
  form. The design doc identifies this as where strict-on-ambiguity lives or
  dies for users.
- *Orphan-rule lint.* Implicits not defined alongside their signature or any
  of their dispatched types produce a warning, not an error — a lint
  signaling likely coherence issues without forbidding the third-party
  extension pattern.

## Dependencies

**Requires:**
- [Stage 3 — First-class modules](module-system-3-first-class-modules.md)

**Unblocks:**
- [Stage 6 — Equivalence-checked coherence](module-system-6-equivalence-checking.md)
- [Stage 7 — Syntax tuning and witness types](module-system-7-syntax-tuning.md)

Stage 4 (axioms) is not a hard prerequisite — modular implicits can ship
without axiom checking — but the cross-implicit equivalence story (stage 6)
combines them.
