# Open work

Future work on the type and module system. Each entry points at a
`roadmap/*.md` item.

## Module-system stages

- [Dependent parameter annotations](../../roadmap/module-system-dependent-param-annotations.md)
  — parameter types that reference earlier parameters in the same FN
  signature; required for OCaml-style multi-parameter functor signatures
  with cross-parameter sharing constraints. Refines the `Deferred(_)`
  carrier from [functors.md § Functors](functors.md) toward precise
  structural function types instead of the current `KType::Any` coarsening.
- [Stage 4 — Property testing and axioms](../../roadmap/module-system-4-axioms-and-generators.md)
  — Rust-side property-testing engine kept disjoint from dispatch; axiom
  syntax in signatures (`AXIOM #(...)` over quoted bool predicates);
  generators-as-required-signature-slots; compile-time axiom checking on
  ascription. Generators thread randomness via the monadic effect surface
  ([design/effects.md](../effects.md)), so this stage requires
  [monadic-side-effects](../../roadmap/monadic-side-effects.md). Independent
  of stage 5's implicit dispatch.
- [Stage 5 — Modular implicits](../../roadmap/module-system-5-modular-implicits.md)
  — implicit module parameters, lexical resolution, strict-on-ambiguity
  policy, explicit-application disambiguation. The "real" generic-code
  ergonomics arrive here, and the multi-parameter dispatch the current
  slot-specificity ranking can't express on its own.
- [Stage 6 — Equivalence-checked coherence](../../roadmap/module-system-6-equivalence-checking.md)
  — cross-implicit equivalence testing using the stage-4 engine. The
  coherence story.
- [Stage 7 — Syntax tuning and witness types](../../roadmap/module-system-7-syntax-tuning.md)
  — disambiguation sugar designed against patterns from real stage-5 code,
  plus opt-in witness types for stronger-than-probabilistic coherence.

## Cross-cutting

- [Standard library](../../roadmap/standard-library.md) — collections built
  as functor FNs over their element/key types. Parks the **applicative
  functor semantics** open question: today's `FN`-based functor surface
  is generative-only, so independent call sites resolving (via stage 5
  implicit search) to the same module still mint distinct output types
  and can't interoperate. Landing form: a separate `FUNCTOR` binder
  reusing FN mechanics, distinguished at the surface so the
  generative/applicative choice is visible at the declaration.
- [Group-based operators](../../roadmap/group-based-operators.md) — paired
  operators like `+`/`-` as a single algebraic declaration. Lands on top of
  the module-system substrate.
- [Static type checking and JIT compilation](../../roadmap/static-typing-and-jit.md)
  — closes the TCO and builtin runtime-check gaps uniformly, and is the
  language's performance ceiling. The compile-time scheduling
  ([scheduler.md](scheduler.md)) — type-returning builtins dispatched and
  bound through the same `Dispatch`/`Bind` machinery values use, with
  implicit search layered as a `SEARCH_IMPLICIT` builtin — is the substrate
  this work builds on.
