# Structural KFunction admission across deferred return types

**Problem.** The structural function-type language coarsens
deferred-return FNs. When a `KFunction`'s `signature.return_type` is
[`ReturnType::Deferred(_)`](../src/machine/model/types/signature.rs),
the structural `KType::KFunction { args, ret }` synthesis at
[`function_value_ktype`](../src/machine/model/values/kobject.rs)
collapses `ret` to `KType::Any` because the structural language has
no surface for "per-call elaboration of this expression." The
symmetric coarsening on the admission side lives at
[`function_compat`](../src/machine/model/types/ktype_predicates.rs) —
when a deferred-return candidate is admission-checked against a slot
typed `:(Function (_) -> SpecificT)`, the comparison reads the
candidate's `ret` as `Any` and the strict `==` refuses admission
silently.

Today's behavior is safe: Stage B never lifts a deferred-return FN
into a structural `KFunction` slot whose `ret` is more specific than
`Any`, so the refusal never fires. A `debug_assert!` at the
coarsening branch of `function_compat` is the tripwire — it fires if
a future test exercises a deferred-return candidate against a
non-`Any` slot, signalling "decide now."

The risk: two FNs differing only in their deferred carriers
synthesize to identical structural `KType`s. Routing today uses
`ReturnType`'s `PartialEq` directly (structure-aware), so admission
and dispatch don't see the collision — but any consumer that reads
the structural `KType` would.

**Impact.**

- *Precise `:(Function (_) -> SpecificT)` slot ascription against
  deferred-return candidates becomes well-defined.* A binding like
  `LET cb :(Function (Er) -> Er) = make_fn` where `make_fn` returns a
  `Deferred(Expression)`-carrying FN admits-or-rejects with a real
  shape comparison, not a silent refusal.
- *Modular-implicit search
  ([Stage 5](module-system-5-modular-implicits.md)) over deferred-
  return candidates becomes decidable.* Implicit resolution searches
  by structural `KType` shape; precision-aware synthesis lets the
  search distinguish candidates that differ only in their deferred
  carriers.
- *Type-class search ergonomics.* Future work on signature-bound
  dispatch over functor-shaped candidates needs to read the
  per-call-elaboration intent, not the coarsened `Any`.

**Directions.**

- *Synthesis side — open.* Two paths for
  `function_value_ktype` when the source FN's `return_type` is
  `Deferred(_)`:
  - *(a) Precision-aware variant.* Mint a new `KType` variant
    (`KFunction` with a `DeferredRet(_)` carrier inside, or a parallel
    `KType::DeferredKFunction`) that records the deferred expression's
    surface form. Equality compares the carriers directly. Requires
    threading the new variant through every consumer of
    `KType::KFunction`.
  - *(b) Value-aware admission.* Leave the synthesized `KType`
    coarsened but route admission through a helper that reaches back
    to the underlying `KFunction::signature.return_type` when the
    candidate is a function value. Lighter on the structural type
    language; doesn't help consumers that only see the synthesized
    `KType` (e.g. structural slot ascription that's already lost the
    `KFunction` carrier).
- *Admission side — open.* The
  [`function_compat`](../src/machine/model/types/ktype_predicates.rs)
  branch's `debug_assert!` is the tripwire — when it fires, the
  decision between (a) and (b) above is forced. Today's strict-`==`
  refusal stays safe until the first non-`Any` slot-ret comparison
  appears, which is gated on either a stage-5 implicit-search
  consumer or a precise FN-typed slot ascription test landing.
- *Bridging today's PartialEq path — decided.* `ReturnType`'s own
  `PartialEq` (used by `signatures_exact_equal` in
  [`bindings.rs`](../src/machine/core/bindings.rs)) stays
  structure-aware regardless of which synthesis path lands — routing
  by signature equality is independent of structural-`KType`
  comparison.

## Dependencies

**Requires:**

**Unblocks:**

- [Stage 5 — Modular implicits](module-system-5-modular-implicits.md)
  — implicit search over functor-shaped candidates whose return
  types reference per-call parameters needs precision-aware
  structural comparison.
