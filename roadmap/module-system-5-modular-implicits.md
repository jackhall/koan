# Module system stage 5 — Modular implicits

**Problem.** Stages 1-2 give an explicit module language: every functor
application, every module-typed argument, every signature constraint is
written by hand. For everyday generic code this is verbose. The
[`KType::Module`](../src/dispatch/types/ktype.rs) slot kind that stage 1
shipped accepts any module value regardless of which signature it satisfies,
so even the explicit-module path lacks the signature-bound dispatch a
generic-function call site needs. Stage 5 introduces **implicit module
parameters**: a function declares that it requires some module satisfying a
given signature, and at the call site the compiler resolves which module to
thread in by searching scope. This is the ergonomic payoff of the design.

**Impact.**

- *Concise generic code.* `sort(xs)` and `MakeSet()` replace
  `sort(IntOrd, xs)` and `MakeSet(IntOrd)`. The compiler resolves which
  module to thread in by searching scope, so call sites stop carrying the
  dictionary by hand.
- *Natural standard-library shape.* `sort`, `min`, `intersect`, `==` take
  their dictionary of operations implicitly and ship as ordinary generic
  Koan code rather than as verbose explicit-module functions or builtins.
- *Multi-parameter dispatch.* Binary operators (`+`, `==`, `intersect`) and other
  multi-type predicates dispatch natively — a multi-type implicit signature
  dispatches on all of its abstract types simultaneously rather than needing a
  partial-order tiebreak between single-type candidates.

**Directions.** None decided.

- *Signature-bound module-typed dispatch.* The substrate the implicit-
  parameter machinery rides on: `KType::Module` slots learn to carry a
  required signature, and the dispatcher checks that the bound module value
  satisfies it. Implicit parameters are then a special case of a typed
  module slot whose argument is resolved by search rather than supplied at
  the call site. Stage 1 shipped the unconstrained `KType::Module` slot;
  this stage tightens it.
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
- [Stage 1.5 — Scheduler integration](module-system-1.5-scheduler.md) —
  modular implicits is the work that forces the `Infer` / `ImplicitSearch`
  scheduler-node story stage 1.5 lands; this stage builds on top of it.

**Unblocks:**
- [Stage 6 — Equivalence-checked coherence](module-system-6-equivalence-checking.md)
- [Stage 7 — Syntax tuning and witness types](module-system-7-syntax-tuning.md)
- [Group-based operators](group-based-operators.md)
- [Static type checking and JIT compilation](static-typing-and-jit.md)

Stage 4 (axioms) is not a hard prerequisite — modular implicits can ship
without axiom checking — but the cross-implicit equivalence story (stage 6)
combines them.
