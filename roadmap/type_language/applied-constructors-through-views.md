# Applied constructor types through views

Using a module's higher-kinded type member from *outside* the module: reads of
applied-constructor-typed value slots through an opaque view, and naming an applied
type over another module's constructor member.

**Problem.** Opaque ascription mints a fresh generative `TypeConstructor` per view for
each `TYPE (Type AS Wrap)` slot, but the value-slot re-tag machinery covers only
first-order abstract members: `body_opaque`'s `slot_type_tags` loop
([`ascribe.rs`](../../src/builtins/ascribe.rs)) tags a slot only when its SIG-declared
type is a bare `KType::AbstractType`, so a VAL slot typed by an *applied* constructor
(`:(Number AS Wrap)`) gets no tag. ATTR's re-tag
([`attr.rs`](../../src/builtins/attr.rs)) never fires for it, and a member value read
through the opaque view reports the source module's constructor identity — the
representation leaks through the abstraction barrier that first-order slot reads
already enforce. Separately, an applied type over a module's constructor member cannot
be *named*: the `AS` builtin's `ctor` slot
([`parameterized_types.rs`](../../src/builtins/parameterized_types.rs)) resolves a bare
Type token against the scope chain, and a dotted `mo.Wrap` inside `:(Number AS
mo.Wrap)` has no type-position elaboration path — even though ATTR already returns the
minted constructor as a first-class type value from `mo.Wrap` in expression position.

**Acceptance criteria.**

- A VAL slot typed with an applied sentinel constructor (`:(Number AS Wrap)`), read
  through an opaque view, reports the view's per-call applied type; passing the read
  value where the source constructor's applied type is expected fails dispatch — the
  barrier holds in both directions.
- `:(Number AS mo.Wrap)` in type position resolves to `ConstructorApply` over `mo`'s
  `Wrap` member — the per-call minted constructor for an opaque view, the source
  constructor for a transparent view or unascribed module.
- A functor parameter's deferred return `-> :(Number AS er.Wrap)` elaborates per call
  and admits values built with the argument module's constructor.

**Directions.**

- *Re-tag mechanism — open.* (a) Extend the `slot_type_tags` mint loop to slots whose
  declared type *contains* a sentinel-constructor application, storing the substituted
  applied type, with ATTR re-tagging the read's `type_id` to it — the mirror of the
  existing `AbstractType` path; (b) rely solely on self-sig substitution and teach
  dispatch to translate at the slot check. Recommended: (a).
- *Dotted-constructor elaboration — open.* (a) Elaborate `mo.Wrap` in `AS`'s `ctor`
  slot through the existing ATTR type-member projection, then apply; (b) a dedicated
  type-expression arm for dotted heads. Recommended: (a).

## Dependencies

**Requires:**


**Unblocks:** none tracked yet.
