# Open work

Future work on the type and module system. Each entry points at a
`roadmap/*.md` item.

## Module-system stages

- [Stage 4 — Property testing and axioms](../../roadmap/predicate_typing/axioms-and-generators.md)
  — Rust-side property-testing engine kept disjoint from dispatch; axiom
  syntax in signatures (`AXIOM #(...)` over quoted bool predicates);
  generators-as-required-signature-slots; compile-time axiom checking on
  ascription. Generators thread randomness via the monadic effect surface
  ([design/effects.md](../effects.md)), so this stage requires
  [monadic-side-effects](../../roadmap/libraries/monadic-side-effects.md). Independent
  of stage 5's implicit dispatch.
- [Stage 5 — Modular implicits](../../roadmap/predicate_typing/modular-implicits.md)
  — implicit module parameters, lexical resolution, strict-on-ambiguity
  policy, explicit-application disambiguation. The call-site witness-elision
  layer over the already-first-class module + signature substrate
  ([modules.md](modules.md)); also lands multi-abstract-type
  implicit resolution for signatures spanning multiple type slots, and generic
  functions as type-parameterized functors selected by implicit resolution
  ([generics.md](generics.md)), including dependent parameters that reference an
  earlier type parameter in the same signature.
- [Stage 6 — Equivalence-checked coherence](../../roadmap/predicate_typing/equivalence-checking.md)
  — cross-implicit equivalence testing using the stage-4 engine. The
  coherence story.
- [Stage 7 — Syntax tuning and witness types](../../roadmap/predicate_typing/syntax-tuning.md)
  — disambiguation sugar designed against patterns from real stage-5 code,
  plus opt-in witness types for stronger-than-probabilistic coherence.

## Cross-cutting

- The module-value chain ([Module naming flip](../../roadmap/type_memos/module-naming-flip.md))
  — modules live on the value channel, typed by the creation-time principal signature under
  canonical signature subtyping. The relation, self-sig, structural dispatch admission, the
  `KObject::Module` value carrier, value-side binding, value-head type paths, and the retirement
  of the type-position `KType::Module` variant have all shipped (see
  [First-class modules](modules.md#first-class-modules) and
  [Module heads in type position](modules.md#module-heads-in-type-position)). What remains is the
  naming flip: module names to value-side snake_case, retiring the Type-token bridge arms in the
  resolver ladder. The combined end state is pinned in
  [module-values-and-type-identity.md](module-values-and-type-identity.md).
- [Abstract member names versus builtin type names](../../roadmap/type_language/abstract-member-names-vs-builtin-types.md)
  — the `Type` member convention this tree teaches (`LET Type = Number`, `TYPE Type`) is
  rejected by the unshadowable-builtins rule
  ([lookup-protocol.md](lookup-protocol.md)). One of the two has to move.
- [Standard library](../../roadmap/libraries/standard-library.md) — collections built
  as FUNCTORs over their element/key types. Parks the **applicative
  functor semantics** open question: the FUNCTOR binder
  ([functors.md](functors.md)) is the decided seam, but applicative
  semantics are deferred behind the predicate-typing work — the language
  stays generative-only until then. Once predicate typing lands, opt-in
  syntax and the identity-by-(functor, arguments) machinery let
  independent call sites resolving (via stage 5 implicit search) to the
  same module interoperate.
- [User-defined operator modules](../../roadmap/operator_chaining/user-defined-operator-modules.md)
  — module-declared operators, including paired/group forms like `+`/`-`. Algebraic
  structures over them (group laws, generic-over-groups) ride
  [modular implicits](../../roadmap/predicate_typing/modular-implicits.md).
- [Two-phase execution](../../roadmap/editor_tooling/two-phase-execution.md)
  — closes the TCO and builtin runtime-check gaps uniformly, and is the
  language's performance ceiling. The build-time scheduling
  ([scheduler.md](scheduler.md)) — type-returning builtins dispatched and
  bound through the same `Dispatch`/dep-finish machinery values use, with
  implicit search layered as a `SEARCH_IMPLICIT` builtin — is the substrate
  the pegged-frontier build phase builds on.
