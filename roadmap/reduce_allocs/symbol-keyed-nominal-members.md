# Symbol-keyed nominal member names

**Problem.** Two name surfaces still carry text where the classified label vocabulary
([design/label-interning.md](../../design/label-interning.md)) says they should carry a
symbol. They have different root causes and are grouped here because they are what is
left of the same sweep.

*The registry's nominal members.* A sealed member's name — and its variant schema's keys —
are owned text inside the interned node.
[`TypeNode::SetMember`](../../src/machine/model/types/node.rs) holds `name: String`, and
`NodeSchema::TypeConstructor` holds `schema: HashMap<String, KType>` beside a
`param_names: Vec<TypeSymbol>` that is already classified. These are the last name-bearing
positions in the type registry that did not move, against a design rule whose opening
sentence is that no per-node owned `String` carries a label. The cost is per *read*, not
per declaration: [`TypeRegistry::node`](../../src/machine/model/types/registry.rs) clones
the node it hands back, so every `SetMember` read heap-allocates the name — and, for a
`TypeConstructor`, clones the whole variant map including every key. That read sits on hot
paths: the satisfaction check in
[ktype_predicates.rs](../../src/machine/model/types/ktype_predicates.rs), the render in
[ktype.rs](../../src/machine/model/types/ktype.rs), and the construction dispatch in
[apply_callable.rs](../../src/machine/execute/decide/apply_callable.rs). Downstream the
text keeps propagating: variant selection compares tag `String`s (`apply_callable.rs`,
`union::variant_repr`), `CtorKind::Tagged`
([constructors.rs](../../src/machine/execute/decide/constructors.rs)) holds an owned `tag`
plus an `Rc<HashMap<String, KType>>` of the cloned schema, `KObject::Tagged` bumps the tag
bytes into the value's region per constructed value, and `PendingMember` copies each name
again at seal
([recursive_group_window.rs](../../src/machine/model/types/recursive_group_window.rs)).

*A classification the parser already made, discarded.* A record literal's field name is
**already** validated into the `BinderSymbol` partition at parse:
[`DictFrame::finish`](../../src/parse/dict_literal.rs) accepts `ExpressionPart::Identifier`
or `ExpressionPart::Type` as a field name and rejects every other part shape. A keyword
lexes as `ExpressionPart::Keyword` and hits that error arm, so "nothing binds to a keyword"
holds there structurally rather than by predicate — and Type-token field names are admitted
*deliberately for the type language*, with `WITH {Elt = …}` named in the comment. The class
is then thrown away: `ExpressionPart::RecordLiteral` is
`&'a [(&'a str, ExpressionPart<'a>)]`, and evaluation lands the fields in a
`RecordSubstrate` keyed by bare `Symbol` — correct on the substrate's own terms, since its
cells are laid out symbol-sorted and binary-searched and no record *value* needs a class.

`WITH` ([type_ops/with.rs](../../src/builtins/type_ops/with.rs)) sits downstream of both
hops. It reads an eager-evaluated `KObject::Record` — deliberately, so a dotted pin value
like `er.Carrier` sub-dispatches in value context for free — so each pin name arrives as a
bare `Symbol` and is round-tripped back through the interner and re-classified with
`TypeSymbol::of`, **twice**: once in the validation walk, once in the fold. `ATTR`
([attr.rs](../../src/builtins/attr.rs)) loses the same information one step earlier.
`read_field_name` takes the field from one of three channels — the value-channel
`Identifier` cell, or the type-channel unresolved/resolved leaf — and *which channel it
came from determines the class*, since dispatch has already sorted the `field:Identifier`
slot before the body runs. All three arms collapse to `String`, and `access_type_member`
then re-derives by running **both** class predicates over that text (`TypeSymbol::of` and
`ValueSymbol::of`) to pick among `manifest_members` / `abstract_members` and `value_slots`.
The classes are disjoint, so the second probe is always dead.

**Ruling (pinned).** The node currency rule is already settled — a member name is a
declared Type-class label, so it is a `TypeSymbol` interned at the declaration that mints
the member, and the text lives only in the run's label interner. A variant tag
(`Ok`, `Error`, `Some`, `None`) and a NEWTYPE name are Type tokens by the same predicate
the rest of the vocabulary uses, so the classified newtype fits without widening it.
Equally settled: a token class established at parse is not re-derived downstream. What is
*not* settled is the canonical member order a component's indices are assigned from, and
which hop of the record-literal pipeline carries the class — see Directions.

**Acceptance criteria.**

- `TypeNode::SetMember.name` is a `TypeSymbol`, and `PendingMember.name` and
  `RecursiveGroupWindow::seal_singleton` take the same currency, so sealing a group copies
  no name text and reading a member node allocates nothing for its name.
- `NodeSchema::TypeConstructor.schema` and its pre-seal twin `RelativeSchema::TypeConstructor.schema`
  key by `TypeSymbol` through the shipped `TypeMemberMap` alias
  ([sig_schema.rs](../../src/machine/model/types/sig_schema.rs)), identity-hashed. Variant
  selection is a `u128` compare, and cloning a node's schema copies no text.
- The tagged-construction path carries the tag as a `TypeSymbol` end to end:
  `CtorKind::Tagged`, `construct_tagged`, and `KObject::Tagged` hold a `Copy` symbol, so
  constructing a tagged value bumps no tag bytes into its region.
- The component digest recipe
  ([type_digest.rs](../../src/machine/model/types/type_digest.rs)) feeds a member's name
  and each variant-schema key as fixed-width symbol bits, sorted by those bits, matching
  the composition the abstract-member and schema feeds already use. The golden pins
  ([type_digest/tests/golden.rs](../../src/machine/model/types/type_digest/tests/golden.rs))
  move for the recursive-group and nominal families and for nothing else — that
  containment is the cross-check, not incidental.
- Every site that renders a member name — `KType::name`, the unknown-variant diagnostic,
  the ill-kinded-constructor diagnostic, module summaries — resolves through the run's
  label interner, with a resolve miss rendering the standard placeholder.
- A record literal's field-name class survives from the parse that established it to the
  seams that consume it: `WITH` validates and folds its pins with **no** symbol→text→symbol
  round-trip and no re-classification, in either pass. Which hop carries the class is the
  open fork below; the criterion is that no consumer re-derives it.
- `read_field_name` returns the field as a classified `BinderSymbol`, taken from the
  channel it arrived on rather than from a predicate over rendered text, and
  `access_type_member` selects its map by matching that variant — running neither class
  predicate. The record-substrate probe in `wrapped_field_cell` keeps its bare
  `Symbol::of`: that is a runtime data-label lookup, where no class is wanted.
- The recorded allocation baselines do not regress, and the drop in per-read node cloning
  is visible in a measured figure from the audit shapes.

**Directions.**

- *Canonical member order — open, and the registry side's one real fork.* A sealed
  member's identity is its component digest plus **its index in that component's canonical
  (name) order** ([node.rs](../../src/machine/model/types/node.rs)), and that order is text
  today. Switching the sort key to symbol bits permutes index assignment within a
  multi-member component, so the same content interns to the same *set* of handles but a
  different member-to-index mapping — the component digest is unchanged, the per-member
  handles are not. Options: (a) sort by symbol, accepting the permutation and re-pinning
  the affected goldens, matching how the schema feeds already order; (b) keep the text sort
  by resolving through the interner at seal only, paying one resolve per member per group
  declaration to hold the current mapping. (a) is the consistent choice and a group's
  members are anonymous to a reader either way, but it is a digest-visible decision.
- *Which hop carries the record-literal class — open, and the record side's fork.* Three
  candidates, and they are not equivalent:
  (a) `ExpressionPart::RecordLiteral` carries `BinderSymbol` instead of `&str`, so the
  parser keeps what it already validated. Fixes every AST consumer; does **not** reach
  `WITH`, which reads an evaluated value.
  (b) `RecordSubstrate` keys by `BinderSymbol`. Reaches `WITH`, but widens every record
  value's key slice and puts a class into the one structure whose whole design rests on
  bare symbol bits — the symbol-sorted cell layout and its binary search.
  (c) `WITH` consumes the record-literal AST part, evaluating pin *values* while pin
  *names* stay syntax. Most faithful to what a `WITH` pin means — a slot name is not a
  runtime label — and it leaves the substrate alone, but it is the largest surface change
  and must preserve the value-context dispatch that makes `er.Carrier` work today.
  (a) and (c) compose; (b) stands alone.
- *`Rc<HashMap<…>>` in `CtorKind::Tagged` — likely deletable, but out of this item's
  claim.* Re-keying removes the text from the clone, not the clone. Whether the ctor kind
  can hold the member handle and re-read the schema at finish instead is a separate
  question about `node()`'s clone-on-read contract, which is the same question the
  registry-wide read path raises.

## Dependencies

**Requires:** none — the classified symbol vocabulary, the `TypeMemberMap` alias, the
declaration-seam interning conventions and the identity-hashed table plumbing are shipped
substrate ([design/label-interning.md](../../design/label-interning.md)).

**Unblocks:** none.
