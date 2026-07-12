# Signature subtyping and self-sigs

A canonical signature-subtyping relation, and a principal signature derived at module
creation, so signature satisfaction is a pure structural check on an immutable carried type.

**Problem.** The module-satisfies-signature check is a membership test against
[`compatible_sigs`](../../src/machine/model/values/module.rs) — a `RefCell<HashSet>` mutated
by each ascription, so a module's admissibility changes over its lifetime and an unascribed
module never matches any signature slot
([`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs)). There is no
signature-to-signature relation: `WITH` pinned-slot agreement is ad-hoc equality against
`type_members`, and nothing can answer "does signature A entail signature B."

**Acceptance criteria.**

- A canonical subtyping relation over signatures exists: `Sub <: Super` iff `Sub` supplies
  every `Super` member (width — `Sub` may carry extra members), each manifest type member
  equal, each abstract member unconstrained, each VAL slot's type compatible.
- Every module carries a principal signature (self-sig) derived at creation from its body;
  it is immutable thereafter.
- Transparent ascription (`:!`) checks satisfaction through the same relation; `WITH` pin
  agreement rides the same relation.

**Directions.**

- *Relation shape — decided.* Record-style width/depth: manifest members equal, abstract
  members unconstrained, VAL slots via the existing function-compat machinery.
- *VAL-slot variance — open.* Exact-type vs covariant depth on value slots; record subtyping
  is the model.
- *Self-sig at creation — decided.* Derived once from the module body; the abstract/manifest
  member distinction makes the derivation well-posed.
- *Phasing — decided.* Foundation phase (carries the risk): the canonical subtyping relation
  over signature schemas. Mechanical phases, each leaving the verify-koan slate green:
  self-sig derivation at module creation (additive — dispatch behavior untouched), `:!` and
  `WITH` rewired onto the relation, tests.
- *Match memo home — deferred.* Ships with a simple memo;
  [Memoized subtype matching](memoized-subtype-matching.md) rehomes it in the digest-keyed
  registry entry.

## Dependencies

**Requires:**

- [Structures and signatures](../../design/typing/modules.md#structures-and-signatures) — the
  relation is defined over the shipped abstract/manifest type-member distinction.

**Unblocks:**

- [Structural satisfaction](structural-satisfaction.md) — dispatch switches to consulting
  the relation and the self-sig.
