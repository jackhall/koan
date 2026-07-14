# Module system stage 5 — Modular implicits

**Problem.** Generic dictionary-style polymorphism already works:
modules are first-class values
([`KObject::Module`](../../src/machine/model/values/kobject.rs)), signatures are
first-class values ([`KType::Signature`](../../src/machine/model/types/ktype.rs)),
and dispatch on `:(Signature Foo)` slots picks behavior by signature
satisfaction. A user can write `(SORT int_ord xs)` today — the witness
module flows through `LET`, ATTR, and function calls like any other value.
What stays verbose is the **witness argument itself**: every generic call
site carries an extra module argument that's redundant given the other
arguments' types. The
[`KType::AnyModule`](../../src/machine/model/types/ktype.rs) wildcard
also accepts any module value regardless of which signature it satisfies,
so the explicit-module path lacks the signature-bound slot a generic call
site needs.

**Acceptance criteria.**

- `sort(xs)` and `MakeSet()` type-check and run, with the compiler resolving
  the witness module by searching scope; the call site carries no explicit
  module argument.
- `sort`, `min`, `intersect`, and `==` are defined as ordinary generic Koan
  code that takes its dictionary of operations through an implicit parameter,
  not as explicit-module functions or builtins.
- A call resolving an implicit whose signature declares multiple abstract
  types (`Type`, `Elt`, `Key`, ...) — a binary operator such as `+`, `==`, or
  `intersect` — picks the witness by aligning all of those types against the
  call site's argument types at once.
- `head(xs)` and `sort(xs)` resolve through the same implicit-resolution
  engine: the purely parametric case
  ([design/typing/generics.md](../../design/typing/generics.md)) reads the
  argument's carried element type directly, and the operation-bearing case
  consults a searched witness.
- A function over "anything that forms a group" is written with a
  `{Gp : GROUP}` implicit parameter — `GROUP` being a binary operator with
  identity and inverse over an abstract `t`, its paired operators declared with
  the [`OP` / `GROUP` surface](../../design/operators.md) — and its
  laws are checked as [stage-4 axioms](axioms-and-generators.md).

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
  functors — module-returning `FN`s — taking one or more `:Type` parameters, not
  only module-parameterized
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
  ([ktype/parameterization-and-variance.md § Variance](../../design/typing/ktype/parameterization-and-variance.md#variance)); the parameter side
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
  existing `Dispatch` / dep-finish machinery stage 2 lands.
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

Stage 4 (axioms) is not a hard prerequisite — modular implicits can ship
without axiom checking — but the cross-implicit equivalence story (stage 6)
combines them.

**Requires:** none — its substrate (the module language and VAL-slot abstract-type
tagging) has shipped.

**Unblocks:**

- [Stage 6 — Equivalence-checked coherence](equivalence-checking.md)
- [Stage 7 — Syntax tuning and witness types](syntax-tuning.md)
- [Two-phase execution](../editor_tooling/two-phase-execution.md)
