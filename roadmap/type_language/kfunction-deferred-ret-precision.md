# Structural KFunction admission across deferred parameter and return slots

**Problem.** The structural function-type language coarsens deferred
parameter and return slots on FNs. When a `KFunction`'s
`signature.return_type` is
[`ReturnType::Deferred(_)`](../../src/machine/model/types/signature.rs),
the structural `KType::KFunction { args, ret }` synthesis at
[`function_value_ktype`](../../src/machine/model/values/kobject.rs)
collapses `ret` to `KType::Any` because the structural language has
no surface for "per-call elaboration of this expression." Once
[type-parameter-binding](type-parameter-binding.md) widens parameter
type slots to the same `ReturnType { Resolved, Deferred }` carrier, the
synthesis Anys out *parameter* slots on the same rule. The symmetric
coarsening on the admission side lives at
[`function_compat`](../../src/machine/model/types/ktype_predicates.rs) —
when a deferred candidate is admission-checked against a slot typed
`:(FN (SpecificArgs) -> SpecificT)`, the comparison reads the
deferred position as `Any` and the strict `==` refuses admission
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

- *Precise `:(FN (_) -> SpecificT)` slot ascription against
  deferred-return candidates becomes well-defined.* A binding like
  `LET cb :(FN (Er) -> Er) = make_fn` where `make_fn` returns a
  `Deferred(Expression)`-carrying FN admits-or-rejects with a real
  shape comparison, not a silent refusal.
- *Modular-implicit search
  ([Stage 5](../predicate_typing/modular-implicits.md)) over deferred-
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
  - *(a) Precision-aware carrier.* Record the deferred expression's
    surface form as a deferred field type inside `KFunction`'s parameter
    record (per the [record substrate](record-substrate.md)). Equality
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
- *Admission side — open.* Function-type admission moves to structural
  record subtyping
  ([record-subtyping](record-subtyping.md)); this item's remaining
  question is how a *deferred* field admits under that relation (it reads
  as `Any` until elaborated). The
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

- [Per-call type-parameter binding in parameter signatures](type-parameter-binding.md)
  — that item widens parameter slots to the `Deferred(_)` carrier and
  extends the existing `debug_assert!` tripwire over parameter-slot
  deferreds. This item covers parameter and return slots in one pass
  once the tripwire fires.
- [Record structural subtyping and projection](record-subtyping.md) —
  function-type admission is redefined as structural record subtyping
  there; this item layers deferred-carrier precision on top.

**Unblocks:**

- [Stage 5 — Modular implicits](../predicate_typing/modular-implicits.md)
  — implicit search over functor-shaped candidates whose return
  types reference per-call parameters needs precision-aware
  structural comparison.
