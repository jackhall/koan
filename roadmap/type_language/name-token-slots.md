# Name-token slots for binder positions

**Problem.** koan classifies a bare token by its first letter at parse: `Carrier`, `Str` and
`Point` become `ExpressionPart::Type`, `x` and `compare` become `ExpressionPart::Identifier`.
That class is a binding rule, not decoration
([design/typing/tokens.md](../../design/typing/tokens.md)) — it is how
`SIG Ordered = ((TYPE Carrier) (VAL compare :Number))` tells a type member from a value member
with no annotation.

A builtin that takes a **name** — not a value, not a type reference — has no slot that accepts
both classes. Slot admission is exact and the two arms are disjoint
([ktype_predicates.rs](../../src/machine/model/types/ktype_predicates.rs)):

```rust
TypeNode::Identifier => matches!(part, ExpressionPart::Identifier(_)),
TypeNode::OfKind(k) => match part {
    ExpressionPart::Type(_) => matches!(k, KKind::ProperType | KKind::AnyType),
    _ => false,
},
```

So an `:Identifier` slot refuses `Ordered.Carrier` and an `OfKind(ProperType)` slot refuses
`p.x`. Two consequences follow, and both are load-bearing today.

*Every name-taking builtin declares its binder position twice.* `ATTR`
([attr.rs](../../src/builtins/attr.rs)) registers six overloads where four would do:
`type_type_field_sig` and `module_type_field_sig` exist only to give a Type-classed field token
a slot, and route to the same bodies as their `:Identifier` siblings. `LET`
([let_binding.rs](../../src/builtins/let_binding.rs)) registers `identifier_sig` and `type_sig`,
identical but for `name: KType::IDENTIFIER` against `name: KType::of_kind(KKind::ProperType)`,
sharing one `body`.

*The only slot that admits a Type-classed name is also the lowering trigger.*
`ExpressionPart::resolve_for` ([ast.rs](../../src/machine/model/ast.rs)) keys its guard on the
same slot type the admission required:

```rust
if let (ExpressionPart::Type(t), KType::PROPER_TYPE | KType::ANY_TYPE) = (self, *slot) {
    return match KType::from_symbol(*t) {
        Some(kt) => Held::Type(kt),        // the name is destroyed
        None => Held::UnresolvedType(*t),  // the token's symbol survives
    };
}
```

`KType::from_symbol` ([ktype_resolution.rs](../../src/machine/model/types/ktype_resolution.rs)) is
a fixed eleven-name table — `Number Str Bool Null List Dict KExpression Type Module Signature
Any`. There is no name→handle index anywhere; the type registry is content-addressed, so every
user-declared name misses the table and survives as `Held::UnresolvedType`. But for those eleven
the binder position receives a resolved handle whose name is gone, and the body must recover it
by **rendering the handle and classifying that rendering** — the pattern
[design/label-interning.md](../../design/label-interning.md) exists to remove. `ATTR` does it in
`read_field_name`'s third channel; `LET` does it at `args.ktype("name")`, plus a guard rejecting
the nodes whose rendering is not a bare name at all:

```rust
Some(name_kt) if matches!(ctx.types().node(name_kt),
    TypeNode::List { .. } | TypeNode::Dict { .. } | TypeNode::KFunction { .. } | TypeNode::Sibling(_)) =>
    return done_err(/* "LET name must be a bare type name" */),
Some(name_kt) => Some(name_kt.name(ctx.registries)),
```

For `LET` the divergence is user-visible. One syntactic form, three outcomes, decided entirely
by whether the name happens to sit in `from_symbol`'s table and what its handle renders as:

```
LET Foo  = Number    → ok
LET Str  = Number    → error: name 'Str' is already bound in this scope
LET List = Number    → error: shape error: LET name must be a bare type name, got `:(LIST OF Any)`
LET Dict = Number    → error: shape error: LET name must be a bare type name, got `:(MAP Any -> Any)`
```

`List` and `Dict` lower to parameterized nodes, so no name can be recovered from the handle and
the diagnostic reports a rendering of the lowered type rather than the token the user wrote.

The root cause is that a name has no way to cross the bind seam with its class *whichever class
it is*. A slot is a `KType`, and whatever it admits reaches a body as a `Held`
([carried.rs](../../src/machine/model/values/carried.rs)), which has three arms — `Object`,
`Type`, `UnresolvedType`. None carries a [`BinderSymbol`](../../src/machine/model/labels.rs):
`UnresolvedType` carries a `TypeSymbol`, so a Type token that misses the builtin table does cross
classified, but an `Identifier` name still arrives on the `Object` arm as text a body must
re-classify, and a builtin-table hit arrives as a resolved handle that has thrown the name away.

**Ruling (pinned).** A binder position denotes a **name**, never a type reference, so it must
never resolve — not against the builtin table, not against scope. The class of that name is the
syntactic class the parser already assigned: an `Identifier` part is a `ValueSymbol`, a `Type`
part is a `TypeSymbol`. It is taken from the part variant, never re-derived by a predicate over
rendered text. Whether a name that collides with a builtin may be bound is the ordinary
already-bound question and is settled elsewhere; what this item removes is the divergence
between spellings that have nothing to do with binding.

**Acceptance criteria.**

- A name-token slot type admits `ExpressionPart::Identifier` and `ExpressionPart::Type` in
  `accepts_part` and no other part shape. It is not an `OfKind(_)`, so `resolve_for`'s
  `PROPER_TYPE | ANY_TYPE` guard does not fire for it and `KType::from_symbol` is never consulted
  for a binder position.
- The bind seam delivers a name-token slot's argument **classified**: a `Held` arm carrying a
  `BinderSymbol` beside the token's source text, with the class taken from the part variant.
  No consumer runs a class predicate over a rendering to recover it.
- `ATTR`'s `field` slot and `LET`'s `name` slot both use it. `type_type_field_sig` and
  `module_type_field_sig` are deleted — `attr.rs` registers four overloads, `let_binding.rs`
  one — and no surviving overload pair becomes ambiguous. The new slot joins
  `is_unconstrained_name` ([ktype_predicates.rs](../../src/machine/model/types/ktype_predicates.rs))
  and carries its entry in the specificity ordering, so a concrete slot still out-specifies it.
- `read_field_name` ([attr.rs](../../src/builtins/attr.rs)) has two channels, not three: the
  `args.ktype("field")` arm, its `kt.name(registries)` render and the `BinderSymbol::of` over
  that rendering are gone. This closes the residual clause of the ATTR criterion that the
  symbol-keyed nominal-member work shipped on two of its three channels.
- `LET`'s body reads its binder name from the classified carrier. The `args.ktype("name")` arm,
  the `TypeNode::List | Dict | KFunction | Sibling` guard and its `"LET name must be a bare type
  name"` error are deleted, because no binder position can receive a resolved handle any more.
- The four `LET` spellings above differ only in whether the name is already bound. No spelling
  reports a rendered lowered type in a diagnostic. Regression tests pin all four.
- Adding a leaf node for the slot takes a fresh digest tag
  ([type_digest.rs](../../src/machine/model/types/type_digest.rs)) and moves **no** existing
  golden pin ([type_digest/tests/golden.rs](../../src/machine/model/types/type_digest/tests/golden.rs)).
- The recorded allocation baselines ([audit/README.md](../../audit/README.md)) do not regress.
  The render this removes is off the hot path, so the expected movement is none.

**Directions.**

- *Surface syntax — start builtin-only.* The slot can exist as a `KType` constructible from Rust
  with no sigil a user can write. No user-authored signature needs it yet, and a sigil is a
  language commitment that cannot be quietly withdrawn. Revisit if user-defined forms ever need
  a binder position.
- *Carrier shape — a fourth `Held` arm, not a reused `KString`.* Reusing
  `Held::Object(KObject::KString)` would land the name as bare text and put every body back to
  `BinderSymbol::of` over it. That is better than classifying a *rendering*, but it still
  re-derives what the parser already knew, and it cannot represent the class for a body that
  needs to branch on it before looking at the text.
- *Interning stays the body's call.* A binder position is a declaration site for `LET`, which
  must intern so the name renders later, and a reference site for `ATTR`, which must not — the
  reference/declaration split in
  [design/label-interning.md](../../design/label-interning.md). So the carrier holds the
  classification and the source text, and each body chooses `declared` or `of`; the seam itself
  never interns.
- *`FN` parameter names are adjacent but out of scope.* The unplanned entry on parameter token
  class ([type_language/README.md](README.md)) is about checking a parameter's class against its
  slot at definition time. It shares this item's premise — the parser's class is authoritative —
  but not its mechanism, and a parameter name is not a slot argument.

## Dependencies

**Requires:** none — the classified symbol vocabulary and the interning conventions are shipped
substrate ([design/label-interning.md](../../design/label-interning.md)).

**Unblocks:** none.
