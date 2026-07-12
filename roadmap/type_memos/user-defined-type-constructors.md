# User-defined type constructors

A koan-source surface for declaring a fresh named type constructor, so a module can
supply a real constructor witness for a higher-kinded signature slot.

**Problem.** No koan declaration produces a real type constructor. The only
`KKind::TypeConstructor` types with a non-sentinel identity are the builtin `Result`
(arity 2, [`result.rs`](../../src/builtins/result.rs)) and the per-call arity-1
constructors opaque ascription mints into a view's `type_members`
([`ascribe.rs`](../../src/builtins/ascribe.rs)). The `TYPE (Type AS Wrap)` declarator
([`type_decl.rs`](../../src/builtins/type_decl.rs)) is SIG-body-only and mints the
*sentinel* placeholder — the abstract slot declaration awaiting a per-call mint, not a
supplyable witness. So a module body has nothing to bind as a constructor member: HK
fixtures fill the slot with a proper-type placeholder (`LET Wrap = Number`) that the
ascription mint discards, and tests needing a real constructor hand-mint one in Rust.
Obtaining a view's minted constructor requires an already-satisfying module, so now that
satisfaction checks kind and arity
([Structures and signatures](../../design/typing/modules.md#structures-and-signatures)),
no koan program can satisfy an arity-1 higher-kinded slot at all — the bootstrap is
circular.

**Acceptance criteria.**

- A koan-source declaration introduces a named type constructor: a
  `KKind::TypeConstructor` type-table entry with a real (non-sentinel) identity in the
  declaring scope.
- The declared constructor applies in type expressions at its arity —
  `:(Number AS Wrap)` over a declared arity-1 `Wrap` yields the applied type.
- A module supplying the declared constructor as a type member satisfies a
  matching-arity higher-kinded SIG slot (`TYPE (Type AS Wrap)`) at ascription.
- The higher-kinded ascription and functor tests declare their constructor witnesses in
  koan source; no test hand-mints a constructor to satisfy an HK slot.

**Directions.**

- *Surface syntax — open.* A parameterized `NEWTYPE` head (`NEWTYPE (Type AS Wrap) = …`,
  reusing the declaration-by-example shape `TYPE` uses in SIG bodies) vs a new top-level
  declarator. `TYPE` itself stays SIG-body-only.
- *Application semantics — open.* What inhabits `(Number AS Wrap)` over a user
  constructor: a nominal wrapper family per instantiation (the `Result` model, values via
  a variant-style constructor) or a purely abstract brand with no value former.
- *Arity — open.* Arity 1 only (parity with `TYPE (Type AS Wrap)` and the `AS`
  application surface) vs general arity. Recommended: arity 1.

## Dependencies

The gap is now live: signature satisfaction checks kind/arity
([Structures and signatures](../../design/typing/modules.md#structures-and-signatures)),
which turns yesterday's placeholder fills into rejections. That rule is what makes a
koan-source constructor surface necessary.

**Requires:** none — the surface is independent substrate.

**Unblocks:** none tracked yet.
