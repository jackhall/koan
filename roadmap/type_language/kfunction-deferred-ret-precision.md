# Structural KFunction admission across deferred parameter and return slots

**Problem.** A *deferred slot* is a function parameter or return type that
is not a fixed `KType` but an expression elaborated per call — a functor
return like `-> Er.Type`, or a parameter slot referencing a type parameter
not yet solved by implicit-functor resolution
([modular implicits](../predicate_typing/modular-implicits.md)). The
structural function type `KType::KFunction { params, ret }` — the shape
dispatch and ascription compare against — has no way to say "elaborated per
call," so it collapses every deferred slot to `KType::Any`. That coarsening
bites at the two sites that handle the structural type:

- *Synthesis* —
  [`function_value_ktype`](../../src/machine/model/values/kobject.rs) builds
  the structural `KType` for a function value. A slot carried as
  [`ReturnType::Deferred(_)`](../../src/machine/model/types/signature.rs)
  is written as `Any`, so two functions differing *only* in their deferred
  slots synthesize to identical structural types. (This item widens
  parameter slots to the same `ReturnType { Resolved, Deferred }` carrier the
  return slot already uses, so one rule covers both.)
- *Admission* —
  [`function_compat`](../../src/machine/model/types/ktype_predicates.rs)
  checks a candidate against a precise slot like
  `:(FN (SpecificArgs) -> SpecificT)`. It reads the deferred return as `Any`;
  function admission is covariant in the return
  ([ktype.md § Variance](../../design/typing/ktype.md#variance)) and
  `Any.is_more_specific_than(_)` is `false`, so a deferred-return candidate
  fills only an `Any`-return slot — it *silently* refuses the precise one.

This does not bite yet: no shipped path lifts a deferred-return FN into a
structural `KFunction` slot whose `ret` is more specific than `Any`, so the
silent refusal never fires. A `debug_assert!` at that branch of
`function_compat` is the tripwire — it fires the first time a deferred
candidate is compared against a non-`Any` slot, signalling "decide the
representation now." Meanwhile routing is unaffected: dispatch and ascription
compare `ReturnType`'s own structure-aware `PartialEq`, not the synthesized
`KType`, so the collision is invisible to them — but any consumer reading the
synthesized structural `KType` would see it.

**Impact.**

- *Precise `:(FN (_) -> SpecificT)` slot ascription against
  deferred-return candidates becomes well-defined.* A binding like
  `LET cb :(FN (Er) -> Er) = make_fn` where `make_fn` returns a
  `Deferred(Expression)`-carrying FN admits-or-rejects with a real
  shape comparison, not a silent refusal.
- *Modular-implicit search
  ([Stage 5](../predicate_typing/modular-implicits.md)) over deferred-
  return candidates becomes decidable.* Implicit resolution selects
  generic functors by structural `KType` shape; precision-aware synthesis
  lets the search distinguish candidates that differ only in their deferred
  carriers, rather than seeing the coarsened `Any`.

**Directions.**

- *Synthesis side — open.* Two paths for
  `function_value_ktype` when the source FN's `return_type` is
  `Deferred(_)`:
  - *(a) Precision-aware carrier.* Record the deferred expression's
    surface form as a deferred field type inside `KFunction`'s parameter
    record (per the [record substrate](../../design/typing/ktype.md#record-fields-and-ktype-hashing)). Equality
    compares the carriers directly. A *parallel* `KType::DeferredKFunction`
    variant is ruled out — it would fork the very `KFunction` arg
    representation the record work unifies.
  - *(b) Value-aware admission.* Leave the synthesized `KType`
    coarsened but route admission through a helper that reaches back
    to the underlying `KFunction::signature.return_type` when the
    candidate is a function value. Lighter on the structural type
    language; doesn't help consumers that only see the synthesized
    `KType` (e.g. structural slot ascription that's already lost the
    `KFunction` carrier).
- *Admission side — open.* Function-type admission is structural function
  subtyping — contravariant params, covariant return (see
  [ktype.md § Variance](../../design/typing/ktype.md#variance)); this item's
  remaining question is how a *deferred* field admits under that relation (it
  reads as `Any` until elaborated, and `Any.is_more_specific_than(_)` is
  `false`, so it fills only an `Any`-return slot today). The
  [`function_compat`](../../src/machine/model/types/ktype_predicates.rs)
  `debug_assert!` stays the tripwire that forces the (a)/(b) decision when
  a non-`Any` slot-ret comparison first appears.
- *Bridging today's PartialEq path — decided.* `ReturnType`'s own
  `PartialEq` (used by `signatures_exact_equal` in
  [`bindings.rs`](../../src/machine/core/bindings.rs)) stays
  structure-aware regardless of which synthesis path lands — routing
  by signature equality is independent of structural-`KType`
  comparison.

## Dependencies

**Requires:**


**Unblocks:**

- [Stage 5 — Modular implicits](../predicate_typing/modular-implicits.md)
  — implicit search over functor-shaped candidates whose return
  types reference per-call parameters needs precision-aware
  structural comparison.
