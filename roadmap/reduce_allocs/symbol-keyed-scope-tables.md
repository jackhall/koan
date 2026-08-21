# Symbol-keyed scope binding tables

**Problem.** A scope's binding tables key by region-bumped text. `Bindings`
([src/machine/core/bindings.rs](../../src/machine/core/bindings.rs)) holds
`BumpBackedMap<'a, &'a str, _>` for its `data`, `types` and `operators` maps, and the SIG
slot table in [src/machine/core/scope.rs](../../src/machine/core/scope.rs) and `Module`'s
member maps ([src/machine/model/values/module.rs](../../src/machine/model/values/module.rs))
do the same. Everything a binding is keyed *by* elsewhere is now a
[`Symbol`](../../src/machine/model/labels.rs) — a signature's parameter schema, a record's
field index, a type digest's field feed — so the frame bind has to translate back across the
seam: `run_user_fn` ([src/machine/core/kfunction/exec.rs](../../src/machine/core/kfunction/exec.rs))
calls `render_label` once per parameter per call, allocating a `String` the table then keys a
bumped copy of. That resolve is the one *symbol→text* reach on a call path: the other
per-call interner touches — the function-value lane interning each named-argument label in
`apply_function`, record literals and constructors interning field names — are text→symbol
interns that allocate only for first-seen text, where the resolve builds a fresh `String` on
every call.

The text keys also cost a byte-wise compare and a string hash per lookup where a `Symbol`
compare is a `u128` equality, and they keep the scope walk's key currency different from the
signature schema's.

The label design this builds on is pinned in
[design/label-interning.md](../../design/label-interning.md).

**Ruling (pinned).** Binding-name identity is classified by token class, enforced by the
type system rather than runtime text checks. Three newtypes over `Symbol` — `ValueSymbol`
(value tokens), `TypeSymbol` (Type tokens) and `KeywordSymbol` (keyword-class tokens per
`is_keyword_token`) — are minted only by constructors that classify their text, so a
`ValueSymbol` and a `TypeSymbol` can never name the same text and the value/type write-door
check (`partition_guard`) is deleted as unrepresentable, not relocated. Operators are
keyword symbols: an operator token is keyword-class, a space-joined operator probe key stays
keyword-class, and the operator table keys by `KeywordSymbol` — the same vocabulary the
dispatch lane's bucket keys adopt downstream. Nothing binds to a keyword: `ValueSymbol`
rejects keyword-class text. This item introduces all three newtypes.

**Acceptance criteria.**

- `ValueSymbol`, `TypeSymbol` and `KeywordSymbol` exist as classified newtypes over
  `Symbol`, minted only from text of their token class, hashing as a single `u128` under
  the identity-hash discipline.
- The scope binding tables — `Bindings`' `data`, `types` and `operators` maps, the SIG
  slot table and `Module`'s member maps — key by the classified vocabulary
  (`ValueSymbol` / `TypeSymbol` / `KeywordSymbol`), with no `&str` key type in any binding
  map.
- `partition_guard` and the cross-kind write probes are gone: a value/type key collision is
  unrepresentable, and the partition `ShapeError`s are raised at the text→symbol
  declaration seam.
- `bind_delivered_direct` takes a `ValueSymbol`, `register_type_direct` takes a
  `TypeSymbol`, and `run_user_fn` binds each parameter straight from the signature schema's
  symbol with no interner reach and no `String` built per parameter per call.
- A name lookup that arrives as source text (a scope walk from an `Identifier` part)
  computes the classified symbol at the seam and compares symbol bits from there down; a
  wrong-class probe statically misses.
- Diagnostics that name an unbound or shadowed binding resolve the text through the run's
  label interner, and a resolve miss renders the same stable placeholder the rest of the
  render paths use.
- The recorded per-call allocation count for a user-defined function with n parameters drops
  by the n `String`s the frame bind builds, measured by differencing an audit shape.

**Directions.**

- *Key type — decided.* The classified newtypes per the ruling above, identity-hashed,
  wrapping the `Symbol` every other label site already carries.
- *Where the text→symbol conversion sits — decided: the lookup seam.* Resolve-ladder
  signatures keep taking `&str`; each converts once at the top (`::of`, a pure hash) and
  walks with the symbol. Needs no AST change; the parse-boundary alternative (an
  `Identifier` part carrying its symbol) stays available to the dispatch item, where the
  probe is hot enough to warrant it.
- *Operator-group keys — decided per the ruling.* Keyword vocabulary: the operator table
  keys by the `KeywordSymbol` of the sorted-joined probe.

## Dependencies

**Requires:** none — `Symbol`, the run-frame label interner and the signature's symbol-keyed
parameter schema are shipped substrate ([design/label-interning.md](../../design/label-interning.md)).

**Unblocks:**

- [Symbol-keyed dispatch buckets](symbol-keyed-dispatch-buckets.md) — reuses the
  `KeywordSymbol` vocabulary and seam conventions minted here for the dispatch bucket keys.
