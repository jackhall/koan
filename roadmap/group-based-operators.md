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

- *Group as a trait.* On top of the user-defined-traits substrate, a `Group<T>` trait
  declares the binary op, its inverse, and an identity. Registering `Number` as
  `Group<Number>` under `+`/`-` is one trait impl; registering `Path` as `Group<Path>`
  under `/`/`..` is another. Operator dispatch consults the trait when no concrete
  overload matches. Most expressive option.
- *Group as a syntax-level shorthand.* `GROUP + - OVER Number` (or similar) registers
  both operators and links them in one declaration, without depending on the trait
  machinery. Less powerful — no generic-over-groups functions — but unblocks "this type
  wants a paired operator" without traits.
- *Group laws.* Math groups have axioms (associativity, identity, inverse). The language
  can either trust the declaration (cheap, possibly wrong) or sample-test it (expensive,
  partial). Trusting is fine if violations only produce wrong answers, not crashes —
  which is the case for a dispatch-only mechanism.
- *Parser surface.* [operators.rs](../src/parse/operators.rs)'s registry is flat today.
  Group declarations would either feed it at runtime (slot allocation deferred to
  dispatch) or extend a compile-time table (structural, rigid). User-definable groups
  force the runtime path.

## Dependencies

**Requires:**
- [`TRAIT` builtin for structural typing](traits.md) — without traits, the syntax-level
  shorthand still works but doesn't unlock the generic-function-over-groups payoff.

Land alongside or after the trait machinery.
