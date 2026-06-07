# First-class record-type sigil

Make `:{…}` a record type the parser and elaborator handle directly, retiring the internal
`RECORD` type-constructor builtin and its desugar.

**Problem.** A `:{…}` record type is not first-class. The parser desugars it to
`SigiledTypeExpr([Keyword("RECORD"), Expression(<field-list>)])`
([`parse/frame.rs`](../../src/parse/frame.rs)) and routes it through an internal `RECORD`
type-constructor builtin (`body_record` / `CarrierKind::Record` in
[`type_constructors.rs`](../../src/builtins/type_constructors.rs)); the elaborator has no
native record case, so every record type — anonymous, nested, or a `NEWTYPE` repr — is
produced only by sub-dispatching that builtin. This collapses the record sigil and the
constructor sigils (`:(LIST OF T)`, `:(MAP K -> V)`) into one `SigiledTypeExpr` part-kind, so
no consumer can match "a record type" structurally. Two costs land in
[`newtype_def.rs`](../../src/builtins/newtype_def.rs)'s record-repr overload: it peeks
`Keyword("RECORD")` to tell a record repr from a constructor-sigil repr, and it carries a
non-record-sigil fallback branch. `typed_field_list.rs` likewise sub-dispatches a `:{…}` field type instead of
elaborating it inline.

**Acceptance criteria.**

- `:{…}` parses to a dedicated record-type AST part, not `SigiledTypeExpr([Keyword("RECORD"), …])`.
- The elaborator produces `KType::Record` from that part directly; the internal `RECORD` builtin
  (`body_record`, `CarrierKind::Record`) no longer exists.
- `NEWTYPE`'s record-repr overload matches the record part structurally — no `Keyword("RECORD")`
  peek and no non-record-sigil fallback branch.
- A `:{…}` field type inside a typed field list elaborates inline, with no sub-dispatch.

**Directions.**

- *Carrier shape — open.* A dedicated `ExpressionPart::RecordType(field_list)` part vs. a flagged
  `SigiledTypeExpr` variant the dispatcher routes specially. Recommended: a dedicated part, mirroring
  the first-class value literals (`ListLiteral` / `DictLiteral` / `RecordLiteral`).
- *Elaborator record case — open.* Whether the field-list elaboration reuses
  `parse_typed_field_list_via_elaborator` from inside `elaborate_type_expr`, or a thinner record-only
  walk. Recommended: reuse the shared parser.
- *Dispatch admission — open.* `:{…}` ceasing to be a `SigiledTypeExpr` changes the speculative-sigil
  admission paths (`resolve_dispatch.rs`, `pick.rs`); the `NEWTYPE` record overload's slot type and
  `FN`'s `body_record_schema` signature path both need re-routing onto the new part-kind.

## Dependencies

Requires: none — the record-repr `NEWTYPE` overload this simplifies has shipped.
