# Consolidate `parse`

The parser owns what it produces: the label vocabulary, the syntax AST, and the form table every
node is classified against; its laws are stated as properties.

**Problem.** `parse` produces the AST and mints every symbol, but both live under `machine::model`.
[labels.rs](../../src/machine/model/labels.rs) holds the interner, the four symbol classes and the
`is_type_name` classifier; the parser imports all of them from `machine`, and `labels` itself
imports `BindKind` from the binder table and the identity hasher from the type registry. The AST
under [ast.rs](../../src/machine/model/ast.rs) mixes syntax with runtime: the file that defines
`KLiteral`, `ExpressionPart` and `KExpression` also lowers a literal to a `KObject`, resolves a part
to a `Held` cell against a slot type and implements `Parseable`.

The node's construction chokepoint fills seven cache fields — bucket key, dispatch shape, operator
probe, binder plan, binder name slot, body layout, lazy-slot stamp — by probing two static tables,
[`BINDER_SPECS`](../../src/machine/model/binder.rs) and
[`LAZY_SLOT_SPECS`](../../src/machine/model/lazy_slots.rs). Two more tables recognize forms the
same way: [close_inference.rs](../../src/machine/model/close_inference.rs)'s `FORM_SPECS` and
[miss_diagnostics.rs](../../src/machine/model/miss_diagnostics.rs)'s `MISS_DIAGNOSTICS`. Of the 26,
41, 30 and 15 keys they spell, 24, 24, 27 and 12 are spelled in at least one other table. Each
table has its own probe, two matchers exist for one comparison (`key_matches_parts` over a
`Part`-bounded generic, `key_matches_untyped` over an owned `Vec`), `announced_type_declaration`
and `op_declaration_arity` re-walk the binder table on nodes that already matched it at seal, and
`binder_name_slot` caches a field of the entry the node already matched.
[`WorkingExpression`](../../src/machine/model/ast/working.rs) mirrors six of the cache fields,
copies them one by one, and re-declares the seven accessors; the owned `untyped_key()` copy on
both nodes has no production caller. Four test files each re-derive the live registration set to
pin their table against it.

The parser's test suite is 409 tests in the touched files, about 290 of them instances of a
general law — "lowering any rendered tree under any layout yields that tree", "every masked index
is a slot position of its key", "a chain's probe is the digest of its operator set" — pinned one
input at a time. No AST → source renderer exists to state the round-trip law.

**Acceptance criteria.**

- `parse` owns the label vocabulary (`Symbol`, `LabelInterner`, the classified symbols,
  `StaticName`, `BindKind`, the identity hasher, `is_type_name`, `is_keyword_token`), the syntax
  AST (`KLiteral`, `ExpressionPart`, `KExpression`, `ProgramExpression`, `ProgramNode`,
  `PartClass`, `DispatchShape`, `KeyElement`), the node cache, the slot layout, and the form table.
- One form table, `FORMS`, spells every builtin form's full bucket key exactly once, tagged by a
  `FormId`; binder facts, lazy slots and the reserved bit ride the entry. The close-inference rules
  and the miss diagnostics are `(FormId, …)` pairs and hold no key. One matcher compares a spec key
  to a stored key; no table reader is generic over a part trait.
- One `NodeCache` (key, shape, probe, form, binder plan) is the cache field of both `KExpression`
  and `WorkingExpression`; its accessors are defined once; no node has a `binder_name_slot` or
  `lazy_slots` field or an owned `untyped_key()` method; `binder_name_slot()` and `lazy_kinds_at()`
  read the cached form.
- `WorkingPart`, `WorkingExpression`, `Part`, `FieldSlot`, `PartSummary`, literal lowering, part
  resolution to a cell, `impl Parseable for KExpression`, the announcement pre-scan and the
  machine-fixed binder names stay in `machine`, the runtime operations as inherent impls on the
  `parse` types.
- Outside `#[cfg(test)]` and doc comments, `parse` imports from outside itself only `source`,
  `memory` and `machine::core::KError`; `machine::model` re-exports no `parse` item.
- The laws in the plan's property list are proptest properties in `src/parse/tests/properties.rs`,
  `src/parse/labels/tests.rs`, `src/parse/ast/tests.rs`, `src/parse/forms/tests.rs`,
  `src/parse/forms/layout/tests.rs` and `close_inference/tests.rs`, over a test-side AST → source
  renderer with a layout tape; every test a property subsumes is deleted; the live registration set
  is derived once in the test tree; the pins that fix a message or a surface rule stay.
- The parser suite, the sexlex property tests, the Miri slate and the full `cargo test` pass.
- [expressions-and-parsing.md](../../design/expressions-and-parsing.md),
  [label-interning.md](../../design/label-interning.md), `TEST.md` and the README's source layout
  name `src/parse/` for the AST, the interner, the node cache and the form table.

**Directions.**

- *Where the node's caches live — decided.* Every cache is a function of the parts run and a
  static table, so the tables move to `parse` with the node. A generic annotation parameter on the
  node, a post-parse rebuild pass and a `Cell`-filled cache were rejected.
- *Runtime impls on moved types — decided.* Inherent impls stay in `machine::model`; no trait
  bridges the split.
- *Shared structural readers — decided.* `classify_dispatch_shape` and `operator_probe_for` read
  the stored key plus the head part's class; `stored_untyped_key` and the matcher take an iterator
  of `KeyElement`; `ExpressionPart` and `WorkingPart` each expose an inherent `key_element`.
- *Form identity — decided.* A `FormId` enum with one variant per entry; a test pins each tag to
  its index. A struct of named forms in the `KEYWORDS` style was the alternative.
- *`admit_bare_type_slots` — decided.* Moves with the table; the parser keeps calling it on the
  unfrozen run, feeding the matcher the parts' key elements.
- *`ast/shape.rs` — decided.* `PartClass`, `DispatchShape` and the readers move; `Part`,
  `FieldSlot` and `PartSummary` stay (they name the working expression and the run registries).
- *Parse error type — deferred.* `KError` stays the parser's error type; the partition is
  [error-handling.md](../../design/error-handling.md)'s.
- *Property scope — decided.* Laws only; a pin that fixes a diagnostic string or a surface rule
  is not rewritten as a property. Round-trip properties run 64 cases each.

## Dependencies

Second of the three-step reshuffle: [`memory`](../../src/memory.rs) shipped first, so the parser
already reaches program storage through it; this comes next, then the type-lattice rewrite.

**Requires:** none — its prerequisite shipped.

**Unblocks:**

- [Rewrite the type lattice under property tests](type-lattice-rewrite.md) — the lattice core's
  label imports come from `parse`, leaving it no edge into `machine::model`.
