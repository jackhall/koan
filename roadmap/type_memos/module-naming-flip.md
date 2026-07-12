# Module naming flip

Module identifiers are snake_case value tokens; signature names keep the Type-token spelling
with no `Sig` suffix.

**Problem.** Module names use the Type-token spelling and `MODULE` installs type-side only
([`module_def.rs`](../../src/builtins/module_def.rs)), so modules — values — occupy the
token namespace whose job is naming things that type fields, and the lexical layer routes
module references through the type channel. Because the binding stays type-side, two
type-channel residuals survive the value-carrier work: the overload-picker probe still
resolves a bare module name to `Carried::Type(KType::Module)`
([`resolve_name_part`](../../src/machine/execute/dispatch.rs), classified by the
`Carried::Type(KType::Module)` arm of `accepts_carried`'s `Signature` case in
[`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs)), and a
signature-typed FUNCTOR parameter's member access projects through a type-side `KType::Module`
([`body_identifier`](../../src/builtins/attr.rs)). Both are consequences of type-side binding
and collapse when `MODULE` binds value-side.

**Acceptance criteria.**

- `MODULE` binds a snake_case identifier in `bindings.data`; declaring a module with a
  Type-token (uppercase-leading) name is an error with a diagnostic.
- Signature names use the Type-token spelling with no `Sig` suffix across stdlib, tests,
  tutorial, and docs.
- Bare module references, ATTR receivers, `USING`, ascription, and value-head type paths
  resolve through the value channel with no `resolve_type_identifier` module bridging.
- The overload-picker probe no longer classifies a bare module name as
  `Carried::Type(KType::Module)`, and `body_identifier` no longer projects a signature-typed
  FUNCTOR parameter's member through a type-side `KType::Module` — both type-channel residuals
  left by the value-carrier work are gone.

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
