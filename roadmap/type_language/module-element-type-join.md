# Module element-type join

**Problem.** With modules riding the value channel as
[`KObject::Module`](../../design/typing/modules.md#first-class-modules), a container
memoizes its element type as the join of its members' types — and
[`TypeRegistry::join`](../../src/machine/model/types/registry.rs) has no `Signature`
arm, so two distinct signature types fall through to `Any`. A list holding module
values with different self-sigs therefore memoizes `List<Any>`, and never satisfies a
`:(LIST OF Ordered)` slot even when every member satisfies `Ordered`. Underneath, the
lattice has no bottom element and no meet, so a join over a contravariant position (a
function's parameters) cannot produce a true upper bound, and empty containers lean on
a boundary-error hack instead of a bottom element type.

**Acceptance criteria.**

- A `Never` bottom type exists: uninhabited, spellable as the builtin name `Never`,
  strictly more specific than every other type, identity element of join and of union
  canonicalization (`:(Never | Number)` is `:Number`).
- A total `TypeRegistry::meet` — greatest lower bound, dual of join — and
  `KType::join`'s `KFunction` arm joins parameters by meet, so a function join is an
  upper bound under the contravariant relation.
- An empty container memoizes `Never` element types (`[]` is `List<Never>`, `{}` is
  `Dict<Never, Never>`) and satisfies every typed container slot. The
  unstamped-empty-container boundary error is retired: `LET empty = []` binds, a bare
  top-level `[]` resolves, and `TYPE OF []` yields `:(LIST OF Never)`.
- `KType::join` of two signature types yields their least common signature supertype
  under the canonical width/depth subtyping relation
  ([module-values-and-type-identity.md](../../design/typing/module-values-and-type-identity.md)):
  members present in both operands — manifest where both operands are manifest and
  equal, abstract at the shared kind otherwise, dropped on kind disagreement — and
  value slots joined pointwise, generalizing to a joined abstract member wherever the
  two slot positions are the operands' respective bindings of it.
- Two signatures sharing no members join to the empty signature — the module-lattice
  top `:Module` — not to `Any`.
- A `LIST` of module values with distinct self-sigs, each satisfying a signature
  `Ordered`, satisfies a `:(LIST OF Ordered)` slot; a test observes the dispatch match.

**Directions.**

- *Join construction — decided.* Width intersection with pointwise member-type join is
  the least upper bound the canonical width/depth relation induces; no new relation is
  introduced.
- *Member-class reconciliation — decided.* Forced by the relation: a manifest
  requirement is satisfied only by an equal manifest binding, an abstract one by any
  binding at the matching kind — hence the manifest/abstract/dropped rule in the
  acceptance criteria. `WITH` pins need no rule of their own: they fold into the
  schema before interning, so a pinned type is ordinary manifest members.
- *Slot generalization — decided.* Value slots anti-unify through the operands'
  bindings of each joined abstract member before falling back to `KType::join`, so a
  slot typed by a demoted member rejoins as a reference to it.
- *Sig-id — decided.* A joined schema uses the canonical `ScopeId::SENTINEL` binder
  every projected SIG uses; demoted members mint nonce-free `AbstractType`s. The
  schema digest ignores `sig_id`, so a joined signature is content-identical to an
  equivalently written SIG declaration, and the empty join lands on `:Module` by
  digest.
- *Bottom type over partial meet — decided.* The lattice gains a real `Never` rather
  than a partial meet confined to function joins.
- *Empty containers — decided.* The infer-error at untyped boundaries is a stand-in
  for a bottom element type; it is removed, not preserved alongside `Never`.

## Dependencies

**Requires:**


**Unblocks:** none tracked yet.
