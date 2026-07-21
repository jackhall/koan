# Module element-type join

**Problem.** With modules riding the value channel as
[`KObject::Module`](../../design/typing/modules.md#first-class-modules), a container
memoizes its element type as the join of its members' types — and
[`KType::join`](../../src/machine/model/types/ktype_resolution.rs) has no `Signature`
arm, so two distinct signature types fall through to `Any`. A list holding module
values with different self-sigs therefore memoizes `List<Any>`, and never satisfies a
`:(LIST OF Ordered)` slot even when every member satisfies `Ordered`.

**Acceptance criteria.**

- `KType::join` of two signature types yields their least common signature supertype
  under the canonical width/depth subtyping relation
  ([module-values-and-type-identity.md](../../design/typing/module-values-and-type-identity.md)):
  the schema whose members are those present in both operands, each member type joined
  pointwise.
- Two signatures sharing no members join to the empty signature — the module-lattice
  top `:Module` — not to `Any`.
- A `LIST` of module values with distinct self-sigs, each satisfying a signature
  `Ordered`, satisfies a `:(LIST OF Ordered)` slot; a test observes the dispatch match.

**Directions.**

- *Join construction — decided.* Width intersection with pointwise member-type join is
  the least upper bound the canonical width/depth relation induces; no new relation is
  introduced.
- *Member-class reconciliation — open.* How the schema's member classes (abstract
  members, manifest members, value slots) pair up across the two operands — whether a
  manifest member in one operand joins with an abstract member of the same name in the
  other, and which class the joined member lands in.
- *Pinned slots under join — open.* A signature type's identity is its schema digest
  plus `pinned_slots`; decide what joining pinned signature types yields — join the
  underlying schemas and drop the pins, or join pointwise over the pinned views.
- *Sig-id of a joined signature — open.* Same-declaration specificity refinement keys
  on a declaring `ScopeId`, and a joined signature is synthesized — no declaration
  exists for it; decide what sig-id it carries — a sentinel, as the empty signature
  uses, or a derived id.

## Dependencies

**Requires:**


**Unblocks:** none tracked yet.
