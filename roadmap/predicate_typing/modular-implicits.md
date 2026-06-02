# Module system stage 5 — Modular implicits

**Problem.** Generic dictionary-style polymorphism already works:
modules are first-class values
([`KType::Module`](../../src/machine/model/types/ktype.rs)), signatures are
first-class values ([`KType::Signature`](../../src/machine/model/types/ktype.rs)),
and dispatch on `:(Signature Foo)` slots picks behavior by signature
satisfaction. A user can write `(SORT IntOrd xs)` today — the witness
module flows through `LET`, ATTR, and function calls like any other value.
What stays verbose is the **witness argument itself**: every generic call
site carries an extra module argument that's redundant given the other
arguments' types. The
[`KType::AnyModule`](../../src/machine/model/types/ktype.rs) wildcard
also accepts any module value regardless of which signature it satisfies,
so the explicit-module path lacks the signature-bound slot a generic call
site needs.

**Impact.**

- *Concise generic code.* `sort(xs)` and `MakeSet()` replace
  `sort(IntOrd, xs)` and `MakeSet(IntOrd)`. The compiler resolves which
  module to thread in by searching scope, so call sites stop carrying the
  dictionary by hand.
- *Natural standard-library shape.* `sort`, `min`, `intersect`, `==` take
  their dictionary of operations implicitly and ship as ordinary generic
  Koan code rather than as verbose explicit-module functions or builtins.
- *Multi-abstract-type implicit resolution.* A signature with multiple abstract
  types (`Type`, `Elt`, `Key`, ...) resolves implicit candidates by aligning all
  of them against the call site's argument types simultaneously, so binary
  operators (`+`, `==`, `intersect`) and other multi-type predicates pick the
  right implicit without ranking single-type candidates against each other.
  Multi-parameter dispatch on declared types is already native to FN; what's
  new is the implicit-search side picking witnesses whose signatures span
  multiple abstract types.
- *Generic functions resolve through one mechanism.* A generic function is a
  functor over its type parameters
  ([design/typing/generics.md](../../design/typing/generics.md)); implicit
  resolution selects and applies it. `head(xs)` and `sort(xs)` become the same
  surface and the same engine — purely parametric generics (no operations on the
  type parameter) and operation-bearing ones (consulting a witness) alike, with
  the parametric case reducing to a direct read of the argument's carried
  element type.
- *Algebraic structures are ordinary signatures.* A `GROUP` signature — a binary
  operator with an identity and inverse over an abstract `t`, its paired operators
  declared via [user-defined operator
  modules](../operator_chaining/user-defined-operator-modules.md) — resolves implicitly, so
  a function over "anything that forms a group" is just one with a `{Gp : GROUP}`
  implicit parameter, its laws checked as [stage-4
  axioms](axioms-and-generators.md). No operator-specific machinery is needed.

**Directions.**

- *Signature-bound module-typed dispatch — decided.* The substrate the
  implicit-parameter machinery rides on: `KType::AnyModule` slots learn
  to carry a required signature (or `KType::SatisfiesSignature` is the
  carrier the slot lowers to), and the dispatcher checks that the bound
  module value satisfies it. Implicit parameters are then a special case
  of a typed module slot whose argument is resolved by search rather
  than supplied at the call site. Stage 1 shipped the unconstrained
  `AnyModule` slot; this stage tightens it.
- *Type-parameterized implicit functors — decided.* Implicit candidates include
  functors taking one or more `:Type` parameters, not only module-parameterized
  ones. The resolver solves such a functor's type argument by reading the call's
  carried argument type (`List(Number)` yields `Number`) — a projection, not a
  search. This keeps the higher-order restriction intact: type arguments come
  from carried types, module arguments come from search, so a `:Type`-parameterized
  implicit functor does not itself take an implicit parameter. The structural
  walk that locates each type parameter inside a parameter slot (`LIST OF Ty`,
  `Result Ty E`, nested containers, a name repeated across slots) lives here,
  matching the slot's elaborated `KType` against the value's carried `KType`.
- *Deferred-parameter type precision — open.* A parameter slot referencing a
  type parameter not yet solved by implicit-functor resolution has no carrier in
  the `params: Record<KType>` storage, so it coarsens to `KType::Any` and admission
  reads `Any` on both sides. The deferred-*return* case already ships its fix — a
  confined `KType::DeferredReturn` shadow carried in the `ret` box, admitted by
  syntactic shadow equality
  ([ktype.md § Variance](../../design/typing/ktype.md#variance)); the parameter side
  wants the contravariant mirror. Recommended: reuse the surface-shadow shape for
  symmetry, decided alongside the resolution that first produces such a slot.
- *Implicit-parameter declaration syntax — open.* The function signature
  needs a slot for implicit module parameters; surface form follows stage
  1's conventions but the exact spelling is unsettled.
- *Explicit-application disambiguation syntax — deferred.* Surface form is
  deliberately deferred to [stage 7](syntax-tuning.md);
  this stage ships a placeholder, and stage 7 designs the user-facing form
  against patterns from real code. The placeholder is intentionally ugly
  so it doesn't accidentally become the final answer.
- *Resolution algorithm — decided per [design/typing/implicits.md § Resolution and coherence](../../design/typing/implicits.md#resolution-and-coherence-the-design-dials).*
  Lexical scope plus explicitly imported implicits; filter by signature
  unification; pick the most specific; ambiguity is an error. Specificity
  rule: most-specific-wins, with unrelated ties as errors.
- *Inference and search interleaving — decided per [design/typing/scheduler.md](../../design/typing/scheduler.md).*
  Implicit search lands as a single `SEARCH_IMPLICIT` builtin — no new
  node kind, no parallel substitution table. Inference produces type
  refinements that search consumes; search produces module choices that
  refine types other inference tasks are waiting on. Both ride the
  existing `Dispatch` / `Bind` machinery stage 2 lands.
- *Higher-order restriction — decided.* Implicit modules cannot themselves
  take implicit parameters; documented and enforced in this stage. This is
  the architectural simplification that keeps resolution decidable and
  search-tree size bounded.
- *Error message investment — decided.* When ambiguity errors fire, they
  name the candidate modules with their import paths and suggest the
  explicit form. The design doc identifies this as where
  strict-on-ambiguity lives or dies for users.
- *Orphan-rule lint — decided.* Implicits not defined alongside their
  signature or any of their dispatched types produce a warning, not an
  error — a lint signaling likely coherence issues without forbidding the
  third-party extension pattern.

## Dependencies

**Requires:** none — its substrate (the module language and VAL-slot abstract-type
tagging) has shipped.

**Unblocks:**

- [Stage 6 — Equivalence-checked coherence](equivalence-checking.md)
- [Stage 7 — Syntax tuning and witness types](syntax-tuning.md)
- [Two-phase execution](../editor_tooling/two-phase-execution.md)

Stage 4 (axioms) is not a hard prerequisite — modular implicits can ship
without axiom checking — but the cross-implicit equivalence story (stage 6)
combines them.
