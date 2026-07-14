# Module naming flip

Module identifiers are snake_case value tokens; signature names keep the Type-token spelling
with no `Sig` suffix.

**Problem.** Module names use the Type-token spelling, so modules — values — occupy the
token namespace whose job is naming things that type fields, and signature names carry a
`Sig` suffix to stay out of their modules' way. Module bindings live value-side, but the
Type-token names keep four marked bridge sites alive, each existing only because a module
name still spells as a type token: the overload-picker probe's Type-token arm
([`resolve_name_part`](../../src/machine/execute/dispatch.rs)), the bare-Type-leaf value
consult ([`bare_type_leaf`](../../src/machine/execute/dispatch/single_poll.rs)), the
Type-token head's self-sig lowering in elaboration
([`elaborate_type_identifier`](../../src/machine/model/types/resolver.rs)), and the
Type-kind placeholder clear a module's value write performs
([`bindings.rs`](../../src/machine/core/bindings.rs)) so an in-flight forward reference
parked on the type ladder still wakes.

**Acceptance criteria.**

- `MODULE` binds a snake_case identifier in `bindings.data`; declaring a module with a
  Type-token (uppercase-leading) name is an error with a diagnostic.
- Signature names use the Type-token spelling with no `Sig` suffix across stdlib, tests,
  tutorial, and docs.
- Module names are snake_case across stdlib, tests, tutorial, and docs; bare module
  references, ATTR receivers, `USING`, ascription, and value-head type paths resolve through
  the ordinary value channel for Identifier tokens.
- The four Type-token bridge sites are deleted: no resolver-ladder arm accepts a
  Type-token-named module, and a module's value write clears no Type-kind placeholder.

**Directions.**

- *Phasing — decided.* Foundation phase: token/binder reclassification — `MODULE` requires
  a snake_case name, the Type-token diagnostic lands, and the bridge arms are deleted.
  Mechanical phases, each leaving the verify-koan slate green: repo-wide rename churn
  (module names to snake_case, the `Sig`-suffix drop) across stdlib, tests, tutorial, and
  docs.
- *`MODULE` remains a declarator — decided.* It binds value-side; anonymous module
  expressions are out of scope.

## Dependencies

**Requires:** none — value-side module binding and value-head type paths have shipped.

**Unblocks:**

- [Functor collapse](functor-collapse.md) — FUNCTOR's type-side home and Type-head application
  surface exist only because a module was a type; both retire once module results bind value-side.
