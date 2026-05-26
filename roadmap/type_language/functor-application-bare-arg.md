# FUNCTOR application with a bare type-language argument

Calling a FUNCTOR with a value-language module that already satisfies the
declared signature should not require a `:!` ascription / parens wrapping
workaround at the application site. Two related surface problems sit
under one root cause: the dispatch boundary's treatment of bare
type-language identifiers in an argument position.

**Problem.** A FUNCTOR's parameter type is typically a signature
(`Er :OrderedSig`); a value-side caller has a module value bound at some
identifier (`IntOrd`, with `compatible_sigs` membership for `OrderedSig`).
The natural application form `(MAKESET IntOrd)` fails dispatch today.
Concrete fallout observed once the FUNCTOR binder
([design/typing/functors.md](../../design/typing/functors.md)) put
user-callable FUNCTORs in reach:

- *Bare argument name dispatches as a Type-class literal, not the bound
  value.* `IntOrd` at an argument position resolves through the
  type-class lookup path before the value-side `IntOrd` is consulted, so
  the dispatcher sees a `KType::Module` literal rather than a
  `Future(KObject::KTypeValue(Module))`. The `(IntOrd)` parens-wrapped
  form forces evaluation as a sub-expression, which then resolves to the
  value binding — so today's workaround in shipped e2e tests is
  `(MAKESET (IntOrd))`.
- *Without `:!` ascription, `compatible_sigs` membership doesn't suffice
  for the signature-typed slot.* `LET IntSet = (MAKESET IntOrd)` (or the
  parens form) fails to match `OrderedSig`'s slot even though `IntOrd`'s
  module value carries the signature in its `compatible_sigs` set,
  because the slot's `matches_value` check goes through
  `KType::SatisfiesSignature` against a carrier whose `compatible_sigs`
  membership predicate isn't being consulted at the dispatch boundary
  for bare-leaf arguments (the value's `ktype()` projects to a bare
  `KType::Module`, not `SatisfiesSignature`). The current shipped
  workaround is `LET int_ord = (IntOrd :! OrderedSig)` to mint a
  signature-pinned ascription view, then `(MAKESET (int_ord))`.

The e2e test
[`tests/functor_binder_e2e.rs`](../../tests/functor_binder_e2e.rs) carries
the `:!` ascription + parens-wrapped workaround pinned at the call site
where this bug surfaces.

**Impact.**

- *FUNCTOR application reads naturally:* `(MAKESET IntOrd)` works when
  `IntOrd`'s value-side module satisfies the FUNCTOR's declared
  signature, with no ascription scaffolding required for the common
  case.
- *Removes the asymmetry between `IntOrd` and `(IntOrd)` at an argument
  position.* Both forms should mean "the value bound under `IntOrd`"
  when the dispatch slot expects a value, with type-class resolution
  reserved for slots that explicitly want a `TypeExprRef`.
- *`compatible_sigs` membership becomes a first-class dispatch admission
  signal* — a module value with `OrderedSig` in its `compatible_sigs`
  admits at a `SatisfiesSignature { sig_id: OrderedSig }` slot without
  the caller minting an ascription view first.

**Directions.**

- *Argument-position lookup precedence — open.* Two candidates: (a) for
  any dispatch slot that admits a value (not a `TypeExprRef`), prefer
  the value-side lookup over the type-class lookup when both resolve to
  the same name; (b) keep the lookup order but have
  `KType::SatisfiesSignature::matches_value` consult the value's
  `compatible_sigs` membership directly when the dispatch boundary lands
  a `KType::Module` carrier in a signature-typed slot. Likely (a) is the
  cleaner fix at the call site; (b) addresses the same problem one
  layer in and may also be needed.
- *Parens form vs bare form parity — open.* If (a) wins, the parens-form
  workaround should become redundant for value lookup; the e2e test in
  [`tests/functor_binder_e2e.rs`](../../tests/functor_binder_e2e.rs) is
  the regression pin for this once the fix lands.
- *Interaction with the type-position sigil — open.* `IntOrd` is
  a legitimate type-language identifier in a type-position slot
  (`:IntOrd` or as a parameter type); the fix must NOT swap to
  value-side lookup in those positions. The boundary is "slot's declared
  `KType` admits a value" — the same predicate used today for
  `accepts_part` vs the type-class path.

## Dependencies

None — the FUNCTOR binder is shipped substrate; this item is the
follow-up bug surfaced once user code could call a FUNCTOR. The e2e
test pinning the workaround at
[`tests/functor_binder_e2e.rs`](../../tests/functor_binder_e2e.rs) is
the regression site once a fix lands.
