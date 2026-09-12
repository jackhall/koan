# Predicate typing

> **Stale as work, kept as requirements.** Written against the old runtime,
> now behind `pending_rewrite`; the items below record what the language needs
> and are retired by the [rewrite](../rewrite/README.md) as it meets them.

The user-facing typing stages — axioms, modular implicits, equivalence-checked
coherence, witness types — that ride on top of the type-language substrate.
The agreed design is captured in [old_design/typing/](../../old_design/typing/README.md);
stages 1 and 2 shipped (the module language: `MODULE`/`SIG` declarators,
`:|`/`:!` ascription, per-module type identity, plus the scheduler-driven
elaborator, `WITH` sharing constraints, and higher-kinded type-constructor
slots, plus runtime type-parameter carriers on `List` / `Dict` / `Result`
values with ascription stamping at the FN return, argument, and `LET`
boundaries).

## Items

Every requirements doc in this retired project.

- [Module system stage 4 — Property testing and axioms](axioms-and-generators.md)
- [Module system stage 6 — Equivalence-checked coherence](equivalence-checking.md)
- [Module system stage 5 — Modular implicits](modular-implicits.md)
- [Module system stage 7 — Syntax tuning and witness types](syntax-tuning.md)
