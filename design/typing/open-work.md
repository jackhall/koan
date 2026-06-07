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
  ([modules.md](modules.md)); also lands the signature-bound module-typed
  dispatch the current `:Module` (`OfKind(Module)`) wildcard lacks, plus multi-abstract-type
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
- [Type values as data carriers](../../roadmap/type_language/type-values-as-data-carriers.md)
  — a user type used as a value rides a `KObject` data carrier rather than the value
  channel's dedicated `Carried::Type(&KType)` arm, so a type-value and an instance value
  of that type share one channel and one `ktype()`-keyed classification. That collapses
  the two parallel kind classifiers — `KKind` (`Proper`/`Module`/`Signature`/`Any`) and
  `NominalKind` (`Tagged`/`Newtype`/`TypeConstructor`) — into one lattice, and their two
  wildcard `KType` variants (`OfKind`, `AnyUserType`) into one kind-matched slot that
  admits by the value's reported kind. Every nominal family is matched by construction,
  retiring the hand-enumerated family list that today leaves `TypeConstructor` unhandled.
- [Two-phase execution](../../roadmap/editor_tooling/two-phase-execution.md)
  — closes the TCO and builtin runtime-check gaps uniformly, and is the
  language's performance ceiling. The build-time scheduling
  ([scheduler.md](scheduler.md)) — type-returning builtins dispatched and
  bound through the same `Dispatch`/`Bind` machinery values use, with
  implicit search layered as a `SEARCH_IMPLICIT` builtin — is the substrate
  the pegged-frontier build phase builds on.
