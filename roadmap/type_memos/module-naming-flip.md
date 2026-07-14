# Module naming flip

Module identifiers are snake_case value tokens; signature names keep the Type-token spelling
with no `Sig` suffix.

**Problem.** Module names use the Type-token spelling, so modules — values — occupy the
token namespace whose job is naming things that type fields, and signature names carry a
`Sig` suffix to stay out of their modules' way. [Value-head type
paths](value-head-type-paths.md) moves module bindings value-side but keeps the Type-token
names, resolved through bridge arms in the resolver ladder (a Type-token part whose
value-side hit is a module resolves to the Object arm in
[`resolve_name_part`](../../src/machine/execute/dispatch.rs)) — interim machinery that
exists only because module names still spell as type tokens.

**Acceptance criteria.**

- `MODULE` binds a snake_case identifier in `bindings.data`; declaring a module with a
  Type-token (uppercase-leading) name is an error with a diagnostic.
- Signature names use the Type-token spelling with no `Sig` suffix across stdlib, tests,
  tutorial, and docs.
- Module names are snake_case across stdlib, tests, tutorial, and docs; bare module
  references, ATTR receivers, `USING`, ascription, and value-head type paths resolve through
  the ordinary value channel for Identifier tokens.
- No resolver-ladder arm accepts a Type-token-named module — the bridge arms left by
  [value-head type paths](value-head-type-paths.md) are deleted.

**Directions.**

- *Phasing — decided.* Foundation phase: token/binder reclassification — `MODULE` requires
  a snake_case name, the Type-token diagnostic lands, and the bridge arms are deleted.
  Mechanical phases, each leaving the verify-koan slate green: repo-wide rename churn
  (module names to snake_case, the `Sig`-suffix drop) across stdlib, tests, tutorial, and
  docs.
- *`MODULE` remains a declarator — decided.* It binds value-side; anonymous module
  expressions are out of scope.

## Dependencies

**Requires:**

- [Value-head type paths](value-head-type-paths.md) — snake_case heads in type position
  need value-head elaboration, and the bridge arms this item deletes are built there.

**Unblocks:** none tracked yet.
