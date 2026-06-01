# Remove positional type syntax and prune TypeParams

Retire the legacy positional parameterized-type form (`:(List X)`,
`:(Dict K V)`) so the keyworded sigil (`:(LIST OF X)`, `:(MAP K -> V)`)
is the single surface, then delete the now-producerless `TypeParams`
arms.

**Problem.** Parameterized types have two surface forms. The keyworded
sigil (`:(LIST OF X)`, `:(MAP K -> V)`, `:(FN … -> X)`) is the canonical
dispatch-routed form, served by the type-constructor builtins in
[`type_constructors.rs`](../../src/builtins/type_constructors.rs). The
legacy positional form (`:(List X)`, `:(Dict K V)`) is interpreted only
by [`try_synth_legacy`](../../src/machine/model/types/typed_field_list.rs),
which synthesizes a `TypeExpr` carrying `TypeParams::List` /
`TypeParams::Function` and elaborates it inline against the field-walker's
threaded elaborator. The parser emits neither variant itself — every
sigil parses to flat `[Type(List), Type(X)]` parts — so `try_synth_legacy`
is the sole runtime producer of `TypeParams::List`, and
`TypeParams::Function` has no runtime producer at all (the `:(FN …)` sigil
routes to `body_fn` → `KType::KFunction`). The result is a second,
parallel elaboration route alongside the dispatcher, plus a
[`TypeParams`](../../src/machine/model/ast.rs) enum whose `List` /
`Function` arms are matched in roughly a dozen sites
([`resolver`](../../src/machine/model/types/resolver.rs),
[`ktype_resolution`](../../src/machine/model/types/ktype_resolution.rs),
[`unify`](../../src/machine/model/types/unify.rs),
[`val_decl`](../../src/builtins/val_decl.rs),
[`let_binding`](../../src/builtins/let_binding.rs),
[`return_type`](../../src/builtins/fn_def/return_type.rs),
[`param_refs`](../../src/builtins/fn_def/param_refs.rs),
[`argument_bundle`](../../src/machine/core/kfunction/argument_bundle.rs))
but fed by a single producer.

**Impact.**

- Parameterized types have one canonical surface: `:(LIST OF X)`,
  `:(MAP K -> V)`, `:(FN … -> X)`. A positional `:(List X)` is rejected
  at dispatch with a migration-pointing diagnostic.
- `try_synth_legacy` retires; every sigil routes through the dispatcher's
  pre-resolution + sub-Dispatch path uniformly. The field-walker keeps
  only `rewrite_threaded_self_refs` (the SCC self-reference pre-resolution),
  hoisted to cover every sigil rather than just the keyworded branch.
- `TypeParams::List` / `TypeParams::Function` lose their last producer, so
  the enum collapses toward a marker and the consumer arms across the
  resolver, `from_type_expr`, `unify`, and the `val_decl` / `let_binding`
  / `return_type` / `param_refs` / `argument_bundle` annotation paths
  delete as dead code.

**Directions.**

- **Reachability sweep — open.** Prove every `TypeParams::List` /
  `TypeParams::Function` match arm is unreachable once `try_synth_legacy`
  is gone before deleting it. The defensive `Function` arms in
  `return_type` / `val_decl` / `let_binding` suggest a prior producer, so
  the dead-code claim needs verification. *Recommended:* instrument each
  arm with a temporary `unreachable!()` and run the full suite, then
  delete the arms that never fire.
- **Positional rejection surface — open.** A positional `:(List X)` parses
  to `[Type(List), Type(X)]`, which classifies as `ConstructorCall`; `List`
  is not a registered `UserType`, so the natural failure is
  `UnboundName(List)`. *Recommended:* emit a dedicated "positional type
  syntax removed; use `:(LIST OF X)`" diagnostic instead, for migration
  clarity.
- **SCC pre-resolution hoist — decided.** Drop the `try_synth_legacy`
  branch in `parse_typed_field_list_via_elaborator`; the
  `rewrite_threaded_self_refs` call becomes the sole sigil path. Mechanical
  — remove the `if let Some(te) = try_synth_legacy(..)` arm and unindent
  the `else`.
- **Test conversion — decided.** Convert executable positional usages
  (`container_types.rs` plus any struct / union / let / val sites) to the
  keyworded form, and delete the `fn_with_invalid_list_arity_errors_at_definition`
  guard: keyworded `:(LIST OF X)` carries a single `elem` slot, so an
  arity error is structurally impossible. The `type_sigil.rs` parser tests
  stay — the parser is unchanged and still emits flat sigil parts.

## Dependencies

**Requires:**

- [Type language via dispatch](../../design/typing/type-language-via-dispatch.md)
  (shipped) — the keyworded type-constructor overloads that make the
  positional form redundant, and the field-walker's
  `rewrite_threaded_self_refs` self-reference pre-resolution that the
  retired `try_synth_legacy` path is unified into.

**Unblocks:** none.
