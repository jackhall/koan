# Module naming flip

Module identifiers are snake_case value tokens; signature names keep the Type-token spelling
with no `Sig` suffix.

**Problem.** Module names use the Type-token spelling and `MODULE` installs type-side only
([`module_def.rs`](../../src/builtins/module_def.rs)), so modules — values — occupy the
token namespace whose job is naming things that type fields, and the lexical layer routes
module references through the type channel.

**Acceptance criteria.**

- `MODULE` binds a snake_case identifier in `bindings.data`; declaring a module with a
  Type-token (uppercase-leading) name is an error with a diagnostic.
- Signature names use the Type-token spelling with no `Sig` suffix across stdlib, tests,
  tutorial, and docs.
- Bare module references, ATTR receivers, `USING`, ascription, and value-head type paths
  resolve through the value channel with no `resolve_type_identifier` module bridging.

**Directions.**

- *Phasing — decided.* Foundation phase (carries the risk): binder/token reclassification —
  `MODULE` binds value-side and the resolver ladder stops routing module names through the
  type channel. Mechanical phases, each leaving the verify-koan slate green: repo-wide
  rename churn (module names to snake_case, the `Sig`-suffix drop) across stdlib, tests,
  tutorial, and docs.
- *`MODULE` remains a declarator — decided.* It binds value-side; anonymous module
  expressions are out of scope.

## Dependencies

**Requires:**

- [Value-head type paths](value-head-type-paths.md) — snake_case heads in type position need
  value-head elaboration.

**Unblocks:** none tracked yet.
