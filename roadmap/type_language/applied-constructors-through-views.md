# Applied constructor types through views

Value members read through an opaque view must inhabit the *view's* types —
per-call minted abstract members and constructors — for every slot shape, on
every read surface.

**Problem.** Opaque ascription mints per-call type members, but the value-slot
re-tag machinery covers only first-order abstract members read through ATTR:
`body_opaque`'s `slot_type_tags` loop
([`ascribe.rs`](../../src/builtins/ascribe.rs)) tags a slot only when its
SIG-declared type is a bare `KType::AbstractType`, and the view's child scope
holds the source module's member values verbatim (the replay is pure seal
duplication, [`bulk_install_from`](../../src/machine/core/bindings.rs)). The
representation therefore leaks through the barrier everywhere else:

- A VAL slot typed by an *applied* constructor (`:(Number AS Wrap)`) gets no
  tag: `view.boxed` reports the source's `:(Wrapper {Type = Number})`, which
  source-side dispatch admits.
- A slot whose type *contains* the abstract member leaks wholesale: a
  function-typed slot (`VAL pure :(FN :{x :Number} -> :(Number AS Wrap))`)
  returns source-typed values when called through the view; a `:(LIST OF
  Carrier)` slot reads as `:(LIST OF Number)` and its elements as bare
  `Number`s; record-typed slots likewise.
- `USING <view> SCOPE` bypasses `slot_type_tags` entirely — the window
  ([`open_module_window`](../../src/machine/core/scope/reach.rs)) borrows the
  view scope's binding table, so even a *first-order* abstract slot (`VAL zero
  :Carrier`) reads concrete inside the block.

The type-position side is already in place: `:(Number AS mo.Wrap)` elaborates
in all three view modes (opaque → the view's minted constructor, generative;
transparent / unascribed → the source constructor), and a functor's deferred
return `-> :(Number AS er.Wrap)` elaborates per call — but none of it is
pinned by tests, and the value-side leak makes the per-call return check fail
for view arguments.

**Acceptance criteria.**

- A VAL slot typed with an applied abstract constructor (`:(Number AS Wrap)`),
  read through an opaque view, reports the view's per-call applied type;
  passing the read value where the source constructor's applied type is
  expected fails dispatch — the barrier holds in both directions.
- A function-typed VAL slot called through an opaque view coerces at the
  boundary in both directions: its results carry the view's types (and fail
  source-side dispatch), and arguments typed by the view's members reach the
  underlying function inhabiting the source's types.
- A list-, dict-, or record-typed VAL slot containing abstract members reads
  through an opaque view at the view's types, elements and fields included.
- Every member read inside `USING <view> SCOPE` reports the same view-side
  type the ATTR read reports — first-order and applied slots alike.
- `:(Number AS mo.Wrap)` in type position resolves to `ConstructorApply` over
  `mo`'s `Wrap` member — the per-call minted constructor for an opaque view,
  the source constructor for a transparent view or unascribed module — and
  tests pin all three modes.
- A functor parameter's deferred return `-> :(Number AS er.Wrap)` elaborates
  per call and admits values built with the argument module's constructor,
  opaque-view arguments included.
- Transparent-view and unascribed-module reads stay concrete, and a view's
  non-SIG members read unchanged.

**Directions.**

- *Coercion site — decided.* Construction-time: the opaque view's child scope
  is born holding coerced member values (each VAL member whose SIG-declared
  slot type substitutes differently under the view's mints is coerced during
  the replay), so ATTR, `USING`, dynamic reads, and functor returns are
  correct by construction. The read-time `slot_type_tags` machinery (the mint
  loop, ATTR's re-tag arm, the raw-self-sig override) retires with it.
- *Coercion walker — decided.* One total `coerce_value` recursing on the
  SIG-**declared** slot type (never on two substituted types in lockstep —
  union interning canonicalizes member order), re-tagging at
  `AbstractType` / `ConstructorApply` positions via `wrapped_peel`, rebuilding
  containers and records with coerced cells and substituted carried types,
  and delegating function positions to the eta-wrapper.
- *Function boundary — decided.* A new `Body::CoercedDelegate { underlying,
  spec }` variant (all-`Copy`; `spec` a region reference to the declared FN
  type plus the two member maps): binds against the substituted signature,
  coerces arguments inward, delegates, coerces the result outward.
- *Keyworded surface — deferred.* The replayed dispatch buckets (calling a
  member by its keyword inside `USING`) stay representation-transparent: a
  SIG cannot declare keyworded members, so there is no slot type to coerce
  against. Deferred to
  [sig-keyworded-surface.md](sig-keyworded-surface.md).
- *Dict keys — decided.* A declared dict key type containing an abstract
  member re-stamps the dict's type only: `KKey`s are concrete scalars with no
  type identity to re-tag. Documented limitation.

## Dependencies

**Requires:** none — the elaboration substrate it builds on has shipped.

**Unblocks:**

- [sig-keyworded-surface.md](sig-keyworded-surface.md) — coercing a keyworded
  member needs the SIG to declare it and this item's coercion machinery.
