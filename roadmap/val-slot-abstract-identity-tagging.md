# VAL-slot value-carrier abstract-identity tagging

**Problem.** A value read from an `:|`-ascribed module's `VAL`-declared slot
carries the underlying value's `KType`, not the per-call abstract identity
the SIG body's declared slot type names. For `SIG WithZero = ((LET Type =
Number) (VAL zero: Type))` plus `MODULE IntOrd = ((LET Type = Number) (LET
zero = 0))` plus `LET int_ord = (IntOrd :| WithZero)`, the ATTR read
`(int_ord.zero)` returns `Number(0)` — the underlying value's `ktype()` is
`KType::Number`, not the fresh per-call `KType::UserType { kind: Module,
name: "Type", scope_id: <int_ord-mint> }` that `:|` minted for
`int_ord.Type`. The functor return-type check in
[`KFunction::invoke`](../src/runtime/machine/kfunction/invoke.rs)'s
Combine-finish closure compares the body's `.ktype()` against the per-call
elaborated return type by structural equality, so a functor declared
`(FN (GET_ZERO Er: WithZero) -> (MODULE_TYPE_OF Er Type) = (Er.zero))`
errors at the per-call return-type check even though the slot value is
semantically a member of the declared abstract type.

The Stage B landing test
[`functor_return_module_type_of_parameter_resolves_per_call`](../src/runtime/builtins/fn_def/tests/module_stage2.rs)
documents this caveat. The test currently pins only the FN-def routing
(registration as `Deferred(_)` succeeds, ascription against the SIG
succeeds); the end-to-end `(GET_ZERO int_ord)` call returning the
underlying `Number(0)` is deferred to this item.

Two adjacent gaps share the same "VAL slot type-identity" theme and ride
along with this work:

- *Structural-form inner-name re-elaboration.*
  [`val_decl.rs`](../src/runtime/builtins/val_decl.rs)'s
  `CarrierForm::Raw` parameterized branch elaborates structural shapes
  like `Function<(Type, Type) -> Number>` via `Elaborator` directly
  against `decl_scope`, then sub-Dispatches each free leaf through
  `value_lookup` if the elaboration parks. The leaf-lookup path
  resolves the *outermost* `Type` reference against the SIG-local
  `LET Type = ...` shadow, but inner positions inside the structural
  shape (`Function<(Type, Type) -> Number>`'s arg slots) are
  elaborated once before the leaf sub-Dispatches complete — the
  shadow on inner names isn't honored. Today no shipped test
  exercises a SIG body that shadows `Type` *and* uses it inside a
  structural form, so the gap doesn't bite, but it'll surface once
  modular implicits force full type-shape checking.
- *`ScopeKind` enum for SIG-body classification.*
  [`val_decl.rs`](../src/runtime/builtins/val_decl.rs)'s
  `enclosing_sig_label` and
  [`let_binding.rs`](../src/runtime/builtins/let_binding.rs)'s mirror
  helper both gate on `scope.name.starts_with("SIG ")`, the literal
  label `sig_def::body` stamps on the SIG decl_scope's child. A
  future scope-naming convention change silently mis-classifies the
  gate; both call sites already carry comments naming this item as
  the home for the cleanup.

**Impact.**

- *End-to-end functor-on-VAL-slot calls become writable.* `(GET_ZERO
  int_ord)` returning a value satisfying the per-call return-type slot
  closes the deferred Stage B landing-test variant.
- *Substrate for axiom checking against VAL-declared slots.*
  Module-system stage 4's axiom engine evaluates quoted predicates
  against module-supplied values; correct identity tagging on the
  slot-read path is a precondition for those quotes to type-check
  against the SIG's abstract `Type`.
- *Substrate for modular-implicits dispatch on VAL-typed values.*
  Stage 5's implicit search dispatches on parameter types; if a VAL
  slot's read carries the underlying type rather than the abstract
  identity, the implicit dispatch sees the wrong key.

**Directions.**

- *Tagging site — open.* Two candidates:
  - (a) *ATTR-time tagging.* The
    [`attr.rs` access path for modules](../src/runtime/builtins/attr.rs)
    inspects the source module's SIG (when the carrier is an
    opaquely-ascribed `KModule`) for a VAL slot named under the requested
    attribute, and wraps the read value with the per-call abstract
    `KType`. Local to ATTR; doesn't require changes to value carriers.
  - (b) *Slot-check carrier recovery.* Relax the per-call return-type
    check in `KFunction::invoke`'s Combine-finish closure to accept
    "value whose underlying type satisfies the declared abstract
    `Type`'s representation" by walking the SIG's `type_members[Type]`
    representation chain. Lighter on ATTR; the check becomes
    representation-aware.
- *Lift vs. wrap — open.* Whether the tagging produces a new
  `KObject::Wrapped`-style carrier with the abstract identity in its
  `ktype()` or extends an existing carrier with a per-site identity
  override. Wrapping interacts with the existing `Wrapped` variant
  used for `NEWTYPE`; an override is more surgical but adds a new
  carrier facet.
- *Re-tagging on cross-module dispatch — open.* When a tagged value
  flows into another functor via parameter binding, the parameter's
  per-call elaboration mints a fresh abstract identity. Whether the
  incoming value's tag is replaced, preserved, or coerced needs a
  decision aligned with stage 5's implicit-search rules.
- *Structural-form inner-name re-elaboration — deferred.* Two
  options: (a) round-trip the structural `TypeExpr` through
  `resolve_for` (or an equivalent per-position re-elaboration) before
  the leaf sub-Dispatches park, so every `Type` reference inside
  `Function<(Type, Type) -> Number>` picks up the SIG-local shadow;
  (b) accept the gap and let modular-implicits' full type-shape
  checking absorb it when stage 5 lands. Deferred to
  [Stage 5 — Modular implicits](module-system-5-modular-implicits.md)
  unless a shipped test forces it sooner; modular implicits owns
  full structural-shape checking and will re-elaborate inner
  positions as part of its dispatch-key construction.
- *`ScopeKind` enum for SIG-body classification — open.* Replace the
  `scope.name.starts_with("SIG ")` string-prefix gate in
  [`val_decl::enclosing_sig_label`](../src/runtime/builtins/val_decl.rs)
  and
  [`let_binding::enclosing_sig_label`](../src/runtime/builtins/let_binding.rs)
  with a `ScopeKind` enum on `Scope` (`Sig`, `Module`, `Function`,
  `Root`, …). One-touch internal refactor — no observable behavior
  change, no new tests beyond keeping the existing gates green. The
  call sites already document this item as the cleanup's home.
  Lands alongside the identity-tagging work since both touch the
  SIG-body classification path.

## Dependencies

**Requires:**

None — the substrate (`VAL` declarator, per-call abstract-identity
minting on `:|`, `Deferred` return-type re-elaboration) is in place;
this item plugs the slot-read carrier identity into that machinery.

**Unblocks:**

- [Stage 4 — Property testing and axioms](module-system-4-axioms-and-generators.md)
  — axiom quotes reference VAL-slot members by name and depend on
  those reads carrying the SIG's abstract type identity for the
  quote-elaboration scope to type-check.
- [Stage 5 — Modular implicits](module-system-5-modular-implicits.md)
  — implicit search dispatches on parameter types; correct identity
  tagging on VAL-slot reads keeps dispatch keys aligned with the
  declared abstract types.
