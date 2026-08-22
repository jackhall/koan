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
Equally settled: a class is never re-derived by a predicate over rendered text. The
canonical member order and the currency `WITH` reads its pins in are settled in
Directions.

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
- `WITH` validates and folds its pins with **no** symbol→text→symbol round-trip and no
  class predicate, in either pass: each pin's bare record-field `Symbol` probes the
  schema's classified member tables by bits, and a hit recovers the stored `TypeSymbol` —
  the witness the SIG declaration minted
  ([design/label-interning.md](../../design/label-interning.md)). The record literal's AST
  arm and the record substrate keep their bare `&str` / `Symbol` currencies.
- `read_field_name` returns the field as a classified `BinderSymbol`, taken from the
  channel it arrived on rather than from a predicate over rendered text, and
  `access_type_member` selects its map by matching that variant — running neither class
  predicate. The record-substrate probe in `wrapped_field_cell` keeps its bare
  `Symbol::of`: that is a runtime data-label lookup, where no class is wanted.
- The recorded allocation baselines do not regress, and the drop in per-read node cloning
  is visible in a measured figure from the audit shapes.

**Directions.**

- *Canonical member order — decided.* A sealed member's identity is its component digest
  plus **its index in that component's canonical order**
  ([node.rs](../../src/machine/model/types/node.rs)); the sort key is the **symbol bits**
  (owner symbol as tiebreak), matching the composition the schema feeds already use. The
  digest feed and index assignment share one order — `member_ref_digest(scc_digest, index)`
  is keyed by fold position, so a text-sorted index beside a symbol-sorted feed is
  incoherent. The permutation within a multi-member component is accepted and the affected
  goldens re-pin; a group's members are anonymous to a reader either way.
- *`WITH` pin currency — decided: recover the class from the schema.* Each pin's bare
  record-field `Symbol` probes `abstract_members` / `manifest_members` by bits and a hit
  hands back the stored `TypeSymbol` — the recovery door
  ([design/label-interning.md](../../design/label-interning.md)). No hop of the
  record-literal pipeline carries a class, because a pin's class authority is the SIG
  member it pins, not the literal. Rejected: carrying `BinderSymbol` in
  `ExpressionPart::RecordLiteral` (the AST renders registry-free — `summarize` and the
  parser's `describe` — and every AST consumer already classifies at its own seam with
  text in hand); keying `RecordSubstrate` by class (the substrate's design rests on bare
  symbol bits); `WITH` consuming the AST part (forfeits the eager-evaluated record that
  makes a dotted `er.Carrier` pin value work for free).
- *`Rc<HashMap<…>>` in `CtorKind::Tagged` — deferred, out of this item's claim.* Re-keying
  removes the text from the clone, not the clone. Whether the ctor kind can hold the
  member handle and re-read the schema at finish instead is a separate question about
  `node()`'s clone-on-read contract, which is the same question the registry-wide read
  path raises.

## Dependencies

**Requires:** none — the classified symbol vocabulary, the `TypeMemberMap` alias, the
declaration-seam interning conventions and the identity-hashed table plumbing are shipped
substrate ([design/label-interning.md](../../design/label-interning.md)).

**Unblocks:** none.
