# Unify the unary-operator registration shape

**Problem.** A unary operator is registered in two places, from two hand-written
copies of one shape. [`type_union::register`](../../src/builtins/type_union.rs)
(lines 94-133) spells out, by hand, exactly the triple
[`op_def`](../../src/builtins/op_def.rs)'s `UNARY OP` finalize synthesizes: a
binary overload `[Slot, Keyword(sym), Slot]`, a keyword-first list overload
`[Keyword(sym), Slot]`, and a single-member `ReductionMode::Unary` group
registered under the symbol. The two drifted apart already — `op_def` derives its
registry key through `Scope::register_group_under_all_subsets`, `type_union` calls
`register_operator_group` directly — so a change to what "a unary operator is
registered as" has to be made twice, and only one of the two sites is reachable
from the surface the language documents.

The bodies cannot be shared and are not the target. `|`'s bodies are **native**:
they build a composite `KType` at the fold brand through `alloc_type_composed`, so
no koan source can express them, while `UNARY OP` synthesizes a koan-AST body. The
binary forms are not even the same *kind* of thing: `op_def`'s is a synthesized
*bridge* (body `sym [left right]`, re-entering the list body), while
`type_union`'s independently unions its two operands. What is duplicated is the
**registration door**, not the substance behind it.

**Acceptance criteria.**

- One registration entry point takes an operator symbol, its two signatures, and
  their bodies, and writes the binary bucket, the list bucket, and the size-1
  `Unary` registry entry — with the registry key derived, not hand-spelled.
- `type_union::register` and `op_def`'s `UNARY OP` finalize both go through it,
  each supplying its own bodies.
- The existing `|` tests and the `UNARY OP` tests pass unchanged: `:(A | B | C)`
  still reduces to one keyword-first call, and a `UNARY OP`'s prefix, infix, and
  two-operand surfaces all reach its list body.

**Directions.**

- *Where the door lives — open.* (a) A `Scope` method beside
  [`register_operator_function`](../../src/machine/core/scope.rs) and
  `register_group_under_all_subsets`, taking the two `(signature, body)` pairs;
  (b) a shared helper in `builtins/op_def.rs` that `type_union` calls, keeping the
  operator-surface knowledge in one builtin. Recommended: (b) — the shape is a
  property of the operator surface, and `type_union`'s registration is a builtin
  spelling that surface natively, not a second `Scope` concern.
- *Scope discipline — decided.* A shape-level dedup (~30 lines), not a unification
  of substance: the bodies stay disjoint, and `|` keeps its native builders.

## Dependencies

Related shipped design: the operator declaration surface
([design/operators.md](../../design/operators.md)).

**Requires:** none — leaf cleanup.

**Unblocks:** none tracked.
