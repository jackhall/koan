# SIG slot explicit-type ascription

**Problem.** SIG slot declarations are *ascription-by-example*: writing
`(LET compare = 0)` inside a signature body declares `compare: Number`
because `0.ktype() = Number`. There is no surface form to declare a
slot whose value-type is the SIG's own abstract `Type` member. The
OCaml equivalent `val zero : t` inside an `ORDERED` signature is
unwritable today — `Type` is the abstract type identity, but no
*value* of that type exists at SIG-declaration time to use as the
ascription example. Transparent ascription `:!` doesn't help:
`MODULE_TYPE_OF` reads from the opaque `type_members` table, so the
fresh per-call `KType::UserType { kind: Module, .. }` minted by `:|`
is the only carrier the per-call elaboration sees.

The Stage B landing test
[`functor_return_module_type_of_parameter_resolves_per_call`](../src/runtime/builtins/fn_def/tests/module_stage2.rs)
exercises the dispatch routing for `(FN (GET_TYPE Er: OrderedSig) ->
(MODULE_TYPE_OF Er Type) = ...)` — registration as
`Deferred(Expression(...))` succeeds where pre-Stage-B errored
"unbound name `Er`" — but cannot exercise an end-to-end body-vs-
annotation pairing because the canonical body shape needs an SIG
`Type`-typed value slot the surface can't express.

**Impact.**

- *Canonical `ORDERED` / `SET` signatures with operations declared
  against the abstract type member become writable.* `val compare : t
  -> t -> int`, `val empty : t`, `val insert : t -> elt -> t` — the
  shapes a standard-library collection signature builds around — get
  a direct koan surface form.
- *End-to-end test pairing for the
  [Stage B `Deferred(Expression)` path](../design/module-system.md#functors)
  closes.* A functor `Make` can construct values of `Er.Type` because
  `Er.Type`-typed fields exist on the input module and the result
  module; the per-call slot check then has well-typed body values to
  match against.
- *Substrate for
  [Dependent parameter annotations](module-system-dependent-param-annotations.md).*
  The canonical `(MAKE T: Type elt: T)` shape needs a way to declare
  `elt`'s slot-type as a reference to `T` — the value-side analogue
  of the same SIG-surface gap.
- *Workaround migration.* Type-class-form return-type-position
  references already work end-to-end via the Stage B `Deferred(_)`
  carrier; the remaining lowercase-identifier workarounds in
  signature bodies (`(LET zero = 0)` standing in for `val zero : t`)
  migrate to a documented form.

**Directions.**

- *Surface form — open.* Three candidates:
  - *(a) Type-position ascription in `LET`.* `(LET zero: Type)` — a
    forward declaration without a value, using the SIG's own abstract
    `Type` member as the slot type. Symmetric with FN parameter
    syntax (`x: Number`); requires LET to accept a type-only form
    without a `= <expr>` clause.
  - *(b) Type-position ascription with a coerced example.*
    `(LET zero: Type = 0)` — explicit type plus a sample value the
    type-checker accepts on opaque grounds. Preserves LET's
    value-binding form; needs ascription rules for "value-shaped but
    type-tagged."
  - *(c) Separate `VAL` keyword.* `(VAL zero: Type)` — matches
    OCaml's `val name : type` directly; orthogonal to LET so
    ascription-by-example continues to mean what it means today. Adds
    a new declarator to the SIG body grammar.
- *Type-checker treatment — open.* Whether the declared slot type
  must literally name `Type` (the SIG's own abstract member) or any
  expression resolving to a `KType` carried by the SIG's
  `type_members` table. The latter generalizes naturally to
  multi-abstract-type signatures (`Type`, `Elt`, `Key`) and to
  higher-kinded slots (`Wrap<Number>`).
- *Interaction with ascription-by-example — open.* Whether the new
  form replaces ascription-by-example for *all* SIG slot
  declarations, or coexists. Today's `(LET compare = (FN ...))`
  pattern carries through to ascription via the FN's structural
  signature; an abstract-type-bearing form could either layer in
  alongside (mixed-mode SIG body) or supersede the example-driven
  path entirely.

## Dependencies

**Requires:**

**Unblocks:**

- [Dependent parameter annotations](module-system-dependent-param-annotations.md)
  — the value-side gap (declaring `elt: T` where `T` is an earlier
  parameter's type) needs the same surface mechanism this item
  designs for SIG slot declarations.
- [Standard library](standard-library.md) — canonical `ORDERED` /
  `SET` signatures with operations against the abstract type member
  are the largest consumer.
