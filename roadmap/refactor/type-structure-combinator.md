# One structural walk over `TypeNode`

Every recursion over a type's children reaches its children through one combinator, so a new
compound variant is covered by construction rather than by seventeen separate edits.

**Problem.** `TypeNode`'s compound arms — `List`, `Dict`, `Record`, `KFunction`, `Union`,
`ConstructorApply`, `Signature` — are descended by seventeen hand-written recursions across seven
files, each re-spelling the same seven arms:

- Nine **unary** walks that rebuild or fold one type:
  `substitute_sig_members`, `canonicalize_binder` and `references_sig_member`
  ([sig_schema.rs](../../src/machine/model/types/sig_schema.rs)); `rewrite_siblings` and
  `collect_siblings`
  ([recursive_group_window.rs](../../src/machine/model/types/recursive_group_window.rs));
  `for_each_user_type_ref`
  ([resolve_type_identifier.rs](../../src/machine/execute/decide/resolve_type_identifier.rs));
  `node_digest` and `canonical_type_digest`
  ([type_digest.rs](../../src/machine/model/types/type_digest.rs)); `KType::write_name`
  ([ktype.rs](../../src/machine/model/types/ktype.rs)).
- Eight **binary** walks that descend two types in lockstep: `TypeRegistry::join` and
  `TypeRegistry::meet` ([registry.rs](../../src/machine/model/types/registry.rs));
  `sig_slot_join`, `sig_slot_meet`, `slot_satisfied_by`, `slot_more_specific_or_equal` and
  `slot_types_equal` (sig_schema.rs); `more_specific_walk`
  ([ktype_predicates.rs](../../src/machine/model/types/ktype_predicates.rs)).

`substitute_sig_members` and `canonicalize_binder` are 75-line near-clones differing only in what
their `AbstractType` leaf does; `sig_slot_join` and `sig_slot_meet` differ only in which of the two
they call at a `KFunction` parameter. Adding a variant means finding all seventeen: the arms that
silently fall through a `_ =>` catch-all are indistinguishable from the arms that deliberately treat
a node as a leaf, so an omission surfaces as a wrong answer rather than a non-exhaustive match.

**Acceptance criteria.**

- A unary rebuild over a type's children is written by supplying the leaf rules alone; the
  combinator supplies the compound arms and re-interns through the registry's existing composite
  doors, so a rebuild that changes nothing returns the input handle.
- A binary lockstep walk is written by supplying the per-arm variance (which of the two directions
  a `KFunction` parameter position takes) and the leaf verdicts; the shared descent over matched
  compound pairs, and the mismatch case, are supplied once.
- Each of the seventeen walks named in Problem is either expressed through a combinator or carries a
  comment naming the property that makes it irreducible.
- A walk that treats `SetMember` or `Signature` as a leaf says so explicitly at its call of the
  combinator rather than by omission — `for_each_user_type_ref`'s member discipline and
  `substitute_sig_members`' descent into a nested signature are both expressible, and the two read
  as opposite choices of the same knob.
- Adding a compound variant to `TypeNode` produces a compile error at every site that must decide
  how to treat it, and no error at a site the combinator covers.

**Directions.**

- *Combinator shape — open.* Two candidates: a `children()` / `map_children()` pair on `TypeNode`
  (shallow, allocation-visible, callers own the recursion), or a pair of driver functions taking
  leaf-rule closures (recursion owned by the combinator). Recommended: the driver pair, since the
  binary walks need the matched-pair descent supplied, which a bare `children()` cannot give.
- *Descent control at `SetMember` / `Signature` — decided.* An explicit per-call knob, not a fixed
  rule: both treatments are live and correct in their own callers, and a fixed rule would force one
  of them back into a hand-written walk.
- *Digest walks — open.* Whether `node_digest` and `canonical_type_digest` join the combinator or
  stay hand-written. Their per-variant domain tags are identity-load-bearing and must never move
  ([type-identity.md](../../design/typing/type-identity.md)), so a derived walk has to keep the tag
  table explicit. Recommended: convert `canonical_type_digest` only, and leave `node_digest`'s tag
  recipe hand-written with a test pinning tag coverage.
- *Value- and part-channel predicates — deferred.* `matches_value`, `accepts_carried`,
  `accepts_part` and `matches_type` match a type against a *non-type* (a value, a parser part), so
  they descend a type's children only through `Union`. They are a different shape and stay as they
  are.
- *Ordering against interning — decided.* No new registry doors: a rebuild re-interns through
  `list` / `dict` / `record` / `function_type` / `union_of` / `constructor_apply` / `signature`
  exactly as the hand-written walks do today, so content addressing keeps a no-op rebuild free.

## Dependencies

[Expression shapes are their own kind of function](../type_language/expression-shapes.md) adds an
`ExpressionShape` node — one more compound arm. Landing this item first means that variant arrives
into a derived framework; landing it second means one more walk set to convert. Neither blocks the
other.

**Requires:** none — a leaf refactor over shipped shapes.

**Unblocks:** none tracked yet.
