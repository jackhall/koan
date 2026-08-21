# Symbol-keyed SigSchema

**Problem.** [`SigSchema`](../../src/machine/model/types/sig_schema.rs) — the
module-signature currency behind `SIG` declarations, module self-sigs and ascription —
keys its three member maps (`abstract_members`, `manifest_members`, `value_slots`) by
owned `String`, and its content digest feeds name *text*
(`schema_content_digest` in
[src/machine/model/types/type_digest.rs](../../src/machine/model/types/type_digest.rs)),
where the record-type digest feeds symbol bits. Every seam between a schema and the
symbol-keyed world around it therefore translates: SIG projection resolves each
`VAL`-slot's `ValueSymbol` back to text through the label interner to build the
schema, ascription classifies schema `String` names back into `TypeSymbol`s to seed a
view scope's type members, and `sig_subtype` compares names by string rather than by
`u128` equality. `AbstractType` constructor `param_names` carry the same owned-text
shape (`Vec<String>` on the registry node).

**Acceptance criteria.**

- `SigSchema.abstract_members` and `manifest_members` key by `TypeSymbol`, and
  `value_slots` keys by `ValueSymbol`, identity-hashed; no `String` key remains in the
  schema currency.
- `schema_content_digest` feeds member identity as fixed-width symbol bits, the same
  hash-of-hashes composition the record-type digest uses, with the sorted feeds
  ordered by symbol.
- SIG projection consumes the slot collector's classified symbols directly, and
  ascription seeds a view's type members from the schema's `TypeSymbol`s directly —
  no symbol→text resolve and no text→symbol re-classification at either seam.
- Rendering a schema member's name (diagnostics, module summaries, `sig_subtype`
  mismatch reports) resolves through the run's label interner, with the standard
  placeholder on a miss.

**Directions.**

- *Key types — decided.* The classified vocabulary in
  [src/machine/model/labels.rs](../../src/machine/model/labels.rs): `TypeSymbol` for type
  members, `ValueSymbol` for value slots. No new newtypes.
- *`AbstractType.param_names` — open.* Constructor parameter names ride the registry
  node as `Vec<String>` and feed the digest as text. Flipping them to symbols follows
  the same pattern, but their token class needs confirming before choosing the
  wrapper (`TypeSymbol` vs unclassified `Symbol`).

## Dependencies

**Requires:** none — the classified newtypes and the seams this item consumes (the SIG
slot collector, view seeding) are shipped substrate
([design/label-interning.md](../../design/label-interning.md)).

**Unblocks:** none — leaf.
