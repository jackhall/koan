# Rewrite the type lattice under property tests

The type lattice is a closed algebra over interned nodes, tested by its laws.

**Problem.** [types/](../../src/machine/model/types.rs) is 16,000 lines across sixteen files, and
its module doc calls it the bottom of the dispatch stack. It is not.
[ktype_predicates.rs](../../src/machine/model/types/ktype_predicates.rs) imports `Held`, `KObject`,
`Carried`, the syntax and working parts, and spliced cells;
[sig_schema.rs](../../src/machine/model/types/sig_schema.rs) imports `Scope` and `ModuleDraft` to
project a declaration into a schema; [signature.rs](../../src/machine/model/types/signature.rs)
imports the AST and region brands;
[typed_field_list.rs](../../src/machine/model/types/typed_field_list.rs) imports spans. The pure
core — the handle, node, digest, registry, kind and record files, plus the structural relations
`satisfied_by`, `is_more_specific_than`, `join`, `meet`, union canonicalization, quantifier
substitution, `join_schemas` and `sig_subtype` — depends only on labels and `ScopeId`, and is about
2,600 lines. It is welded to the admission and elaboration glue around it, which is the part that
changes whenever values or the AST change: nearly every commit that touches the directory since it
was created touches that glue.

The structure inside the core repeats itself. Seventeen hand-written recursions descend the same
seven compound arms ([one structural walk](type-structure-combinator.md)); five substitution walks
re-derive relations the composition already answers ([substitute, then ask](substitution-walk-collapse.md)).
Its tests are 260 hand-written shape pins that fix particular inputs and outputs; none states a
law, so a rewrite of any walk has no oracle beyond the shapes someone thought to write down.

**Acceptance criteria.**

- A lattice core exists whose imports are the label and symbol types from `parse`, `ScopeId`, and
  nothing else: no `Scope`, no value or cell type, no AST or working part, no execute-side type.
- The core's public relations are `satisfied_by`, `is_more_specific_than`, `join`, `meet`,
  `union_of`, the quantifier substitutions, `join_schemas` and `sig_subtype`, plus interning,
  rendering and node reads; every caller outside the core reaches types through them.
- Admission — `matches_value`, `matches_held`, `accepts_carried`, `accepts_working_part`,
  `accepts_part`, `slot_ktype` — lives with the values and parts it inspects and calls the core's
  type-level relations; dispatch behavior is unchanged.
- `project_decl` and `raw_self_sig` leave the schema file for the callers that hold the scope and
  draft they project.
- Every structural recursion in the core goes through one unary combinator or one binary lockstep
  combinator, meeting the acceptance criteria of [one structural walk](type-structure-combinator.md);
  the substitution relations are the substitution composed with the ordinary predicate, meeting the
  acceptance criteria of [substitute, then ask](substitution-walk-collapse.md).
- A `proptest` suite over generated type trees pins the lattice laws: `join` and `meet` are
  commutative, associative, idempotent and absorb each other; `is_more_specific_than` is a partial
  order and `a ≤ b` holds exactly when `join(a, b) = b`; `union_of` is insensitive to member order and
  idempotent; interning content-equal nodes twice yields one handle; substituting a binding list
  onto a type with no quantified position returns the input handle; erasing after instantiating
  returns the quantified type; rendering a type parses back to the same handle.
- The golden digest tests stay: the hard-coded handle constants and the digest tag table are pinned
  by value, since a property cannot see a recipe change that reissues every constant.
- Hand-written in-module tests that a law now covers are deleted; the ones that remain each pin a
  shape the laws cannot express, and say which.
- The full `cargo test`, the Miri slate, and the seam-equivalence check pass.

**Directions.**

- *Organizing principle — open.* Whether the core is a `TypeNode` enum with a unary and a binary
  combinator, or a trait implemented per type form with the walk as trait methods. Recommended: the
  enum with combinators — the relations are binary and structural, the digest recipe wants an
  explicit tag table, and interning is simplest over one enum. A trait earns its place only where
  two representations share one walk, which today is the relative pre-seal schema against the
  absolute node schema.
- *Generator strategy — open.* Property tests need arbitrary type trees over a live registry,
  including abstract members and quantified positions that need a `ScopeId` and a binder. Either a
  bespoke `Strategy` that interns as it builds, or generate a syntax tree and lower it through the
  elaborator. Recommended: the bespoke strategy, since the elaborator is outside the core.
- *Test layout — decided.* Properties live in `tests/properties.rs` under the core, next to the
  golden module, following [sexlex](../../sexlex/src/tests/properties.rs) and
  [cellgraph](../../cellgraph/src/graph/tests/properties.rs).
- *Expression shapes against signatures — deferred.* `ExpressionSignature` and the
  `ExpressionShape` node are two representations of a function's shape, and collapsing them is what
  lets keyworded dispatch simplify. That reaches into the function picker and dispatch resolution,
  so it is a follow-up item written once the core is in place, not part of this rewrite.
- *Declaration windows — deferred.* The two window representations stay as they are; merging them is
  [one declaration-window representation](one-declaration-window.md).

## Dependencies

Third of the three-step reshuffle: `memory`, then the parse consolidation, then this. It subsumes
[one structural walk](type-structure-combinator.md) and [substitute, then ask](substitution-walk-collapse.md):
their acceptance criteria are met by the rewritten core, so they retire with it.

**Requires:**

- [Consolidate `parse`](consolidate-parse.md) — the core imports labels from `parse`, which is what
  leaves it with no edge into `machine::model`.

**Unblocks:** none tracked yet.
