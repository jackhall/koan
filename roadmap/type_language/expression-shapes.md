# Expression shapes are their own kind of function

An expression shape — a keyworded, positional definition reached by dispatch — is declared with
`EXPR` and carries a type that records its keywords and argument positions, so it is no longer
spelled or typed as if it were a lambda.

**Problem.** Koan has two kinds of function and one word for both. A **lambda** is an anonymous
callable reached by name, taking a record of named arguments and returning a value; its type is
written `:(FN :{x :Number} -> Bool)`. An **expression shape** is a keyworded definition —
`FN (PURE x :Carrier) -> Carrier = (x)` — reached by dispatch on the keyword/argument sequence that
forms its bucket key, never by name. The second is the first plus a layer: a fixed sequence of
keywords interleaved with argument positions.

Three consequences of the conflation, all live:

- **The type drops the layer.** A signature's keyworded member
  ([design/typing/modules.md](../../design/typing/modules.md)) has as its type an interned
  `KFunction` node — a record of named argument types plus a return
  ([sig_schema.rs](../../src/machine/model/types/sig_schema.rs)). A record canonicalizes its field
  order, and the keywords are nowhere in it. So `(PURE x :Number) -> Number` and
  `(HIDE x :Number) -> Number` are one type, and satisfaction, digests, `TYPE OF`, rendering and
  `fn_type_specificity` all compare shapes that differ on the surface as if they were the same. The
  untyped key is carried *beside* the type, as the map's key, rather than being part of it.
- **The declaration surface is `FN` twice over.** The bodyless declarator shares its dispatch bucket
  key `[FN, Slot, ->, Slot]` with the function-*type* expression `FN :{…} -> <Ret>`
  ([parameterized_types.rs](../../src/builtins/parameterized_types.rs)), separated only by a
  lazy-slot staging entry that captures the head raw
  ([lazy_slots.rs](../../src/machine/model/lazy_slots.rs)). The two forms mean unrelated things and
  the reader has to know which context they are in to tell them apart. The diagnostic shows the
  cost: a malformed function type such as `:(FN (x :Number) -> Bool)` reaches the declarator and is
  advised to *"write `FN (<head>) -> <Return> = (<body>)` to define a function"*, which is wrong
  advice inside a type sigil.
- **A shape cannot be named as a type at all.** There is no spelling for "the type of the
  expression shape `(PURE _ :Number) -> Number`". A `VAL` slot can hold a lambda; only a SIG's
  keyworded channel can talk about a shape, and only by declaring one.

**Acceptance criteria.**

- A SIG declares a keyworded member with `EXPR` —
  `SIG S = ((TYPE Carrier) (EXPR (PURE x :Carrier) -> Carrier))` — and the bodyless `FN` head no
  longer declares one. `FN` in a type position is unambiguously a lambda type again, and the
  bucket-key collision and its misdirected diagnostic are both gone.
- An expression shape's type carries its keyword and argument sequence alongside the argument types
  and the return. Two shapes agreeing on parameter names and types but differing in keyword
  placement are different types — distinct digests, distinct rendering, distinct `TYPE OF` — and a
  shape type is never equal to a lambda type.
- Every consumer of a keyworded member's type reads the shape from the type rather than from the
  map key it is stored under: satisfaction, `join_schemas`, `fold_pins`, specificity, and the
  view's install path.
- A shape type is spellable in a type position, so the same type can be written down outside a SIG
  body, and a slot that expects one refuses a lambda.
- Satisfaction and the ascription view select over **one** representation of a shape and reach the
  relation through **one** entrance. The view installs the overload the satisfaction check chose —
  carried as the selected shape type, which is stable across the content-identity memo in a way a
  bucket index is not — rather than re-deriving candidates from the live bucket table and re-running
  the selection against them. The `debug_assert!` in `install_keyworded_surface`
  ([registry.rs](../../src/machine/core/scope/registry.rs)) guarding a disagreement between the two
  goes away because the disagreement is unrepresentable, not because it is checked.
- The abstract-member substitution has one entrance too. Today the check substitutes per compare
  (`slot_satisfied_by`, given the binder and the source's bindings) while the view pre-substitutes
  the declared type through the coercion plan's `from` side and asks the plain structural rule; a
  divergence between those two routes is a silently narrowed view.

**Directions.**

- *`EXPR` at the definition site — open.* This item's ruling is scoped to the declaration form. A
  definition is still written `FN (PURE x :Carrier) -> Carrier = (x)`, so the word `FN` still names
  both kinds there, and `EXPR (PURE x :Carrier) -> Carrier = (x)` would finish the separation. It is
  a much larger surface change — every koan program, tutorial and snippet — and it is not decided
  here.
- *Where the shape lives in the type — open.* Either extend the `KFunction` node with the untyped
  key, so one node type covers both kinds and a discriminant separates them, or mint a distinct
  type node for a shape and leave `KFunction` to lambdas. The first keeps one satisfaction path and
  one specificity fold; the second makes "a lambda is not a shape" a type-level fact rather than a
  field compare.
- *Where the selection is carried — open.* Either the satisfaction verdict grows a payload (the
  selected shape type per declared member) that the view reads, or the view re-runs a selection that
  is now provably total because both sides read one type family. The first is exact; the second
  keeps the verdict a boolean and leans on the representation alone.
- *Argument order — open.* Once the keyword sequence is in the type, the argument record's
  canonicalized order is redundant with it for a shape, but the record is also what
  `fn_type_specificity` pairs by name. Whether the shape's positions or the record's names are the
  pairing rule needs settling.

## Dependencies

The keyworded surface this re-spells and re-types is shipped —
[design/typing/modules.md](../../design/typing/modules.md) covers the declaration form, an
overload's identity, satisfaction as a dispatch-mirrored most-specific pick, and the ascription
barrier's treatment of it.

**Requires:** none — foundation.

**Unblocks:** none tracked yet.
