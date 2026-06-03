# Named-argument calls consult signature satisfaction for signature-typed params

**Problem.** The named-argument bind path does not consult signature
satisfaction for a `:Signature`-typed parameter, so a module that *satisfies* a
signature is rejected when passed by name. A keyword-led call
(`MAKESET IntOrd`) admits `IntOrd` against an `Er :OrderedSig` slot because
strict admission tests the module's `compatible_sigs` membership through
[`function_compat`](../src/machine/model/types/ktype_predicates.rs). The
named-argument form rebuilds the call without that check:
[`reconstruct_positional`](../src/machine/core/kfunction.rs) maps each
`{name = value}` field onto its declared parameter slot and binds the value
directly, and the eager-subs bind that follows compares the argument's carried
type to the slot type by identity. A `:(MyFunctor {base = IntOrd})` where
`base :OrderedSig` therefore fails with `expected OrderedSig, got IntOrd`, even
though `IntOrd` satisfies `OrderedSig` — the same argument the keyword form
admits. The `:(MyFunctor {…})` head and the `f {…}` function-value call both
route through this named-argument path
([apply_callable.rs](../src/machine/execute/dispatch/apply_callable.rs)'s
`Function` arm), so both surfaces carry the gap.

**Impact.**

- *Functors and functions over signature-typed module parameters apply through
  the named-argument surface.* A `:(MyFunctor {base = IntOrd})` application
  admits any module whose `compatible_sigs` contains the slot's signature, the
  same way the keyword-led `MAKESET IntOrd` form does today.
- *Head-position functor application closes for signature-typed params.* With
  the named-argument path consulting satisfaction, head-position functor
  application is total over signature-typed parameters, not only simple-typed
  ones — the modular-implicits use case where a witness module is threaded
  through a `:Signature` slot by name.

**Directions.**

- *Align the named-argument bind-time check with the keyword-led path — open.*
  The keyword-led path admits a `:Signature` slot via
  [`function_compat`](../src/machine/model/types/ktype_predicates.rs)'s
  `compatible_sigs` membership test before binding. The named-argument bind
  needs the same satisfaction check rather than identity comparison. Whether the
  fix routes the reconstructed call back through the strict-admission predicate,
  or has the post-reconstruction eager-subs bind consult satisfaction directly,
  is the core decision.

## Dependencies

**Requires:** none — builds on shipped substrate: the named-argument bind path
and the `compatible_sigs` satisfaction check.

**Unblocks:** none.
