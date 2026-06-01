# VAL-slot ATTR re-tagging

Adjustment within the ATTR-on-type machinery: when ATTR reads a VAL-declared
slot from an opaquely-ascribed module, wrap the returned value with the
per-call abstract identity the SIG names instead of the underlying value's
concrete `KType`.

**Problem.** A value read from an `:|`-ascribed module's `VAL`-declared
slot carries the underlying value's `KType`, not the per-call abstract
identity the SIG body's declared slot type names. For `SIG WithZero =
((LET Type = Number) (VAL zero :Type))` plus `MODULE IntOrd = ((LET Type
= Number) (LET zero = 0))` plus `LET int_ord = (IntOrd :| WithZero)`,
the ATTR read `(int_ord.zero)` returns `Number(0)` — the underlying
value's `ktype()` is `KType::Number`, not the fresh per-call
`KType::AbstractType { source_module: <int_ord-module>, name: "Type" }`
that `:|` minted for `int_ord.Type`. The functor return-type check in
[`KFunction::invoke`](../../src/machine/core/kfunction/invoke.rs)'s
Combine-finish closure compares the body's `.ktype()` against the per-call
elaborated return type by structural equality, so a functor declared
`(FN (GET_ZERO Er :WithZero) -> (MODULE_TYPE_OF Er Type) = (Er.zero))`
errors at the per-call return-type check even though the slot value is
semantically a member of the declared abstract type.

The Stage B landing test
[`functor_return_module_type_of_parameter_resolves_per_call`](../../src/builtins/fn_def/tests/functor/deferred_return.rs)
pins the FN-def routing; the end-to-end `(GET_ZERO int_ord)` call
returning the underlying `Number(0)` is what this item closes.

**Impact.**

- *End-to-end functor-on-VAL-slot calls become writable.* `(GET_ZERO
  int_ord)` returns a value satisfying the per-call return-type slot,
  closing the deferred Stage B landing-test variant.
- *Substrate for axiom checking against VAL-declared slots.*
  Module-system stage 4's axiom engine evaluates quoted predicates
  against module-supplied values; correct identity tagging on the
  slot-read path is a precondition for those quotes to type-check
  against the SIG's abstract `Type`.
- *Substrate for modular-implicits dispatch on VAL-typed values.* Stage
  5's implicit search dispatches on parameter types; if a VAL slot's
  read carries the underlying type rather than the abstract identity,
  the implicit dispatch sees the wrong key.

**Directions.**

- *Tagging site — decided.* The `KType::Module` arm of ATTR-on-type
  ([`attr.rs`](../../src/builtins/attr.rs)'s `access_module_member`)
  performs the re-tagging. On a VAL-slot read, the arm looks up the
  slot's declared `:Type` in the source SIG, resolves `Type` through the
  module's `type_members`, and wraps the returned value's carrier with
  that abstract identity. The source `&Module` pointer and the per-call
  frame are both in hand at the ATTR call site (the carrier is
  `KType::Module { module, frame }`), so the work is local to
  `access_module_member` and stays inside that arm.
- *Wrap vs. override — open.* Whether the re-tagging produces a new
  `KObject::Wrapped`-style carrier with the abstract identity in its
  `ktype()`, or extends an existing carrier with a per-site identity
  override. Wrapping interacts with the existing `Wrapped` variant used
  for `NEWTYPE`; an override is more surgical but adds a new carrier
  facet.
- *Structural-form inner-name re-elaboration — deferred.*
  [`val_decl.rs`](../../src/builtins/val_decl.rs)'s `CarrierForm::Raw`
  parameterized branch elaborates structural shapes like `:(FN
  (Type, Type) -> Number)` via `Elaborator` directly against `decl_scope`,
  then sub-Dispatches each free leaf through `value_lookup` if the
  elaboration parks. The leaf-lookup path resolves the *outermost* `Type`
  reference against the SIG-local `LET Type = ...` shadow, but inner
  positions inside the structural shape (`:(FN (Type, Type) ->
  Number)`'s arg slots) are elaborated once before the leaf sub-Dispatches
  complete — the shadow on inner names isn't honored. Today no shipped
  test exercises a SIG body that shadows `Type` *and* uses it inside a
  structural form, so the gap doesn't bite. Deferred to
  [Stage 5 — Modular implicits](../predicate_typing/modular-implicits.md)
  unless a shipped test forces it sooner; modular implicits owns full
  structural-shape checking and will re-elaborate inner positions as
  part of its dispatch-key construction.

## Dependencies

**Requires:**

**Unblocks:**

- [Stage 4 — Property testing and axioms](../predicate_typing/axioms-and-generators.md)
  — axiom quotes reference VAL-slot members by name and depend on those
  reads carrying the SIG's abstract type identity for the
  quote-elaboration scope to type-check.
- [Stage 5 — Modular implicits](../predicate_typing/modular-implicits.md)
  — implicit search dispatches on parameter types; correct identity
  tagging on VAL-slot reads keeps dispatch keys aligned with the
  declared abstract types.
