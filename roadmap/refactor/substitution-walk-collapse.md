# Substitute, then ask

The signature-slot relations are the substitution composed with the ordinary predicate, not five
hand-written walks that recompute what the composition would answer.

**Problem.** [sig_schema.rs](../../src/machine/model/types/sig_schema.rs) carries five walks whose
own doc comments define them as a composition they then avoid performing:

- `slot_satisfied_by` — "verdict of `substitute_sig_members(declared, sig_id, members).satisfied_by(sub_type)`
  … computed without materializing the substituted type".
- `slot_more_specific_or_equal` — the same for `is_more_specific_than`.
- `slot_types_equal` — the same for handle equality.
- `references_sig_member` and `substitution_binding` — the two probes that exist only to route the
  three above onto their fast paths.

Together they are roughly 400 lines re-deriving `satisfied_by`, `is_more_specific_than` and
structural equality one arm at a time. The avoidance was worth a rewrite when interning was
expensive. It is not now: `TypeRegistry` is a digest-keyed HAMT whose intern is insert-if-absent,
`is_more_specific_than` memoizes every verdict unconditionally in `verdicts`
([registry.rs](../../src/machine/model/types/registry.rs)), and a substitution that changes nothing
returns the input handle. The walks concede the point themselves — all three materialize anyway at a
nested `Signature`, each with its own copy of the reason.

The duplication has already drifted. `slot_more_specific_or_equal` hand-copies the top guards of
`more_specific_walk` ([ktype_predicates.rs](../../src/machine/model/types/ktype_predicates.rs)) —
including the four members of that file's private `is_unconstrained_name` — but not its `Never`
guards, its `Str`-versus-bare-token ordering, or its exclusion of an unconstrained name on the
subject side. Whether any divergent case is reachable past the fast paths that precede it is not
established; that it cannot be read off either file is the point.

Separately, sig_schema.rs is four concerns in one 1,650-line file: the `SigSchema` carrier, the
`sig_subtype` relation, the `join_schemas` lattice, and the `MemberCoercion` / `CoercionTables`
plan.

**Acceptance criteria.**

- `slot_satisfied_by`, `slot_more_specific_or_equal` and `slot_types_equal` each survive as the
  named entrance their callers use, and each is the substitution composed with the corresponding
  ordinary predicate — no second structural descent over `TypeNode` exists for any of the three.
- `references_sig_member` and `substitution_binding` are gone, or survive only as an
  identity-substitution fast path measured to pay for itself.
- A property test over generated schema/slot pairs asserts the composition and the shipped verdicts
  agree, and runs before the walks are deleted.
- The unconstrained-name and `Never` guard set is stated in exactly one place; no file outside
  `ktype_predicates.rs` enumerates the members of `is_unconstrained_name`.
- `sig_subtype`'s and the ascription view's substituting comparisons reach the relation through the
  same entrance, as
  [expression shapes](../type_language/expression-shapes.md) requires, and that entrance is the
  composition.
- A benchmark over a signature-heavy program shows no regression attributable to the materialized
  substitution.

**Directions.**

- *Fast path — open.* Whether to keep a cheap "does this type reference any substituted member"
  guard so a concrete slot skips the rebuild entirely. Recommended: measure first — the rebuild of
  a member-free type is already a walk that returns the input handle, so the guard may be buying
  nothing over content addressing.
- *Order against the combinator — decided.* Independent of
  [one structural walk over `TypeNode`](type-structure-combinator.md), and cheaper first: deleting
  five walks is five fewer to convert. Landing the combinator first is also valid and costs only the
  conversion work this item then discards.
- *File split — decided.* Split sig_schema.rs into a `sig_schema/` directory along the four
  concerns named in Problem, in the same change: the deletion is what brings the file within reach
  of a split that is otherwise mostly churn.
- *Verification order — decided.* The property test lands and passes against the current walks
  first, so it is a differential check rather than a restatement of the new implementation.

## Dependencies

**Requires:** none — a leaf refactor over shipped relations.

**Unblocks:** none tracked yet.
