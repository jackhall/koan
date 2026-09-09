# Consolidate `parse`

The parser owns what it produces: the label interner and symbol types, and the syntax half of the
AST. The working expression stays with the interpreter.

**Problem.** `parse` produces the AST and mints every symbol, but both live under
`machine::model`. [labels.rs](../../src/machine/model/labels.rs) holds the interner, the four symbol
classes and the `is_type_name` classifier; the parser imports all of them from `machine`. The AST
under [ast.rs](../../src/machine/model/ast.rs) mixes syntax with runtime: the same file that defines
`KLiteral`, `ExpressionPart` and `KExpression` also lowers a literal to a `KObject`, resolves a part
to a `Held` cell against a slot type, reads binder plans and lazy-slot specs, and implements
`Parseable`. [ast/shape.rs](../../src/machine/model/ast/shape.rs) keys parts against dispatch key
elements and spliced cells; [ast/working.rs](../../src/machine/model/ast/working.rs) is the
partially evaluated working expression, runtime state through and through. So `parse` imports the
interpreter to name its own output, and the interpreter's import of the AST is indistinguishable from
its import of its own working state.

`labels` also carries two small back-edges into `machine`: it imports `BindKind` from the binder
table, and its tests use the identity hasher from the type registry.

**Acceptance criteria.**

- `parse` owns the label interner, the symbol types, `is_type_name`, `is_keyword_token`, and the
  syntax AST: `KLiteral`, `ExpressionPart`, `KExpression`, `ProgramExpression`, `ProgramNode`, and
  the part-class and summary types that describe them without a runtime.
- `WorkingPart`, `WorkingExpression` and the dispatch-shape types stay in `machine`; the runtime
  operations on syntax nodes (literal lowering, part resolution to a cell, binder plans, lazy-slot
  specs, `Parseable`) stay in `machine` as inherent impls on the moved types.
- `parse` imports from outside itself only `source` and `memory`, plus whatever the error-type
  direction below settles.
- `labels` imports nothing from `machine`; the binder-kind and identity-hasher edges are cut.
- The parser test suite, the sexlex property tests, and the full `cargo test` pass unchanged.
- [expressions-and-parsing.md](../../design/expressions-and-parsing.md) and
  [label-interning.md](../../design/label-interning.md) name `src/parse/` for the AST and the
  interner, and the README's source layout matches.

**Directions.**

- *Runtime impls on moved types — decided.* Inherent impls may live in any module of the crate, so
  `to_kobject`, `resolve`, `binder_plan` and their peers stay in `machine::model` as impl blocks on
  the `parse` types. No trait is introduced to bridge the split.
- *Parse error type — open.* The parser reports through `KError`, which lives in `machine::core`.
  Alternatives: keep that one import, or give `parse` its own error type that `machine` converts at
  the boundary. Recommended: keep the import for this item and settle the error partition under
  [error-handling.md](../../design/error-handling.md) separately.
- *`admit_bare_type_slots` — open.* A post-parse pass over parts, defined in the binder table
  ([binder.rs](../../src/machine/model/binder.rs)) and called from the parser. Either it moves into
  `parse` as a syntax rewrite or the parser stops calling it and the interpreter applies it on entry.
  Recommended: the interpreter applies it, so `parse` does not read the binder table.
- *`ast/shape.rs` — open.* `PartClass` and `FieldSlot` are syntax; the `Part` trait is implemented
  by both the syntax part and the working part; `DispatchShape` keys on dispatch key elements.
  Recommended: `PartClass` and `FieldSlot` move, the trait and `DispatchShape` stay.

## Dependencies

Second of the three-step reshuffle: `memory`, then this, then the type-lattice rewrite.

**Requires:**

- [A top-level `memory` module](memory-module.md) — the parser's program-storage import must land
  on `memory` for `parse` to stop importing `machine::core`.

**Unblocks:**

- [Rewrite the type lattice under property tests](type-lattice-rewrite.md) — the lattice core's
  label imports come from `parse`, leaving it no edge into `machine::model`.
