# Symbol-keyed field lists

**Problem.** Field and parameter name lists re-derive symbols from text that the parser already
classified. A record literal's keys are `&'a str`
(`ExpressionPart::RecordLiteral(&[(&str, ExpressionPart)])`,
[src/machine/model/ast.rs](../../src/machine/model/ast.rs)), so `schedule_record_literal`
([src/machine/execute/decide/literal.rs](../../src/machine/execute/decide/literal.rs)) interns
every field name on every evaluation, and the record-constructor and named-type-argument doors
(`constructors.rs`, `apply_callable.rs`) mint per field per call. `parse_pair_list`
([src/parse/triple_list.rs](../../src/parse/triple_list.rs)) — STRUCT / SIG / FN / UNION field
lists — builds a `String` per name, runs an O(n²) string dedup, and hands the `String`s to
`typed_field_list`, which interns them ("the one intern site for field labels"), and to the
tag/member binders, which mint again. The FN definition path collects parameter names as
`Vec<String>` (`fn_def/signature.rs`), rendering a Type-token parameter back out of the interner
to do it, and scans a return surface by hashing each of those names to compare it against the
return token's symbol (`fn_def/param_refs.rs`); the anonymous-FN path resolves each schema field's text from the
interner only to re-classify it. `FROM` (`record_projection.rs`) interns each named field per
evaluation.

**Acceptance criteria.**

- `RecordLiteral` keys are `BinderSymbol`s (a key is an Identifier or a Type part) minted at
  parse; record construction, record-newtype construction and named type arguments read the key's
  symbol and mint nothing per evaluation.
- `parse_pair_list` yields `(BinderSymbol, T)` pairs, dedups by symbol, and builds no `String`;
  `typed_field_list` interns nothing; `pair_list_names` and `parse_hk_decl` yield symbols.
- FN parameter names travel as `BinderSymbol`s from signature parse through the return-surface
  scan; the anonymous-FN schema path pushes the schema's symbols directly.
- `FROM` reads each field's carried symbol and dedups by symbol.
- `symbols_minted` and the recorded allocation baselines both drop on the user-FN and
  tagged-construct shapes (a `String` per field / parameter per declaration is gone); no baseline
  regresses.

**Directions.**

- *Record-key class — decided.* `BinderSymbol`: the parser admits Identifier and Type parts as
  keys, `WITH {Elt = …}` probes the Type class, and `.symbol()` gives record construction its bare
  `Symbol`.

## Dependencies

**Requires:**

- [Parse-interned identifiers](parse-interned-identifiers.md) — both part kinds must be
  symbol-only before a field list can be.

**Unblocks:** none.
