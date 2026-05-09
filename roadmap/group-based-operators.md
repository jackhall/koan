# Group-based operators

**Problem.** Operators like `+`/`-` (additive group over Number), `*`/`/` (multiplicative
group over Rationals), and `/`/`..` (path-join + parent-dir over filesystem paths) form
*mathematical groups* — paired binary ops with an identity and an inverse. Today each
operator is a flat builtin registered independently; the language has no concept that
`+` and `-` come as a pair, that `Path` could declare its own group under different
operators, or that a function over "anything that forms a group" could be written
generically. Every new operator-bearing type duplicates registration and re-derives
dispatch correctness in the user's head.

**Directions.**

- *Implementation strategy — open.* Two shapes:
  - *Group as a signature.* A `GROUP` signature declares the binary op, its inverse,
    and an identity over an abstract type `t`. `IntAdd : GROUP with type t = Number`
    ascribes one structure under `+`/`-`; `PathJoin : GROUP with type t = Path`
    ascribes another under `/`/`..`. Modular implicits resolve which group module a
    given operator call uses, so a function over "anything that forms a group" is just
    one with a `{G : GROUP}` implicit parameter. Most expressive option, and falls out
    of the module system without operator-specific machinery.
  - *Group as a syntax-level shorthand.* `GROUP + - OVER Number` (or similar)
    registers both operators and links them in one declaration, without depending on
    modular implicits. Less powerful — no generic-over-groups functions — but unblocks
    "this type wants a paired operator" before stage 5 lands.
- *Group laws — open.* Math groups have axioms (associativity, identity, inverse). The
  signature variant slots into the property-testing engine from
  [stage 4](module-system-4-axioms-and-generators.md) — the laws become axioms checked
  at ascription. The shorthand variant has to trust the declaration, which is fine if
  violations only produce wrong answers, not crashes (the case for a dispatch-only
  mechanism). Falls out of the strategy choice above.
- *Parser surface — open.* [operators.rs](../src/parse/operators.rs)'s registry is flat
  today. Group declarations would either feed it at runtime (slot allocation deferred
  to dispatch) or extend a compile-time table (structural, rigid). User-definable
  groups force the runtime path.

## Dependencies

**Requires:**
- [Module system stage 5 — Modular implicits](module-system-5-modular-implicits.md) —
  for the generic-over-groups payoff. The syntax-level shorthand could ship earlier
  against stage 1's signature surface, but the most expressive variant needs implicit
  resolution.
