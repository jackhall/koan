# A tenant's own scratch

A tenant that writes its host's region but owns a scratch habitat, shared with
its own tenants and handed back at its release.

**Problem.** A tenant owns no storage: a step in it is handed its host's
region writer and its host's scratch writer, and a scratch state or receipt
run it parks lies in its host's bump ([tenant.rs](../src/tenant.rs)). What a
tenant step writes at `'scratch` therefore outlives the tenant, until the
host's scratch next resets, and the host's scratch header, which every later
step of the host and of its other tenants reads, may hold it. A tenant
borrows a half of its own ([tree cells](../src/tree/README.md#death)), and
`'lent: 'scratch` holds only if that lend outlives the host's scratch, so its
count passes to the host's scratch at the tenant's release and drops at that
scratch's reset: a region an embedder releases stays in place until its
borrower's host's scratch resets, which for a host holding its scratch is
the host's death, and a lender that borrows the host's scratch itself closes
a lend cycle that never disposes (`CellGraph::lend_cycle`). And a header a tenant needs
laid a particular way in its scratch, such as one whose content links its
host's region, is the host's to lay first.

**Acceptance criteria.**

- A tenant can be created with a scratch habitat of its own. A step in it,
  and in every tenant created naming it, writes the host's region at `'here`
  and that scratch at `'scratch`, with that scratch's own header.
- That scratch resets at the first step end of the tenant or of a tenant of
  it that leaves nothing naming it, as a write home's does: its own scratch
  state and receipt run, or any of its tenants'. A child's scratch delivery
  to it lands there.
- At its release the scratch is handed back whole and the tenant's lend count
  drops; nothing is left counted on its host's scratch.
- A tenant created without one behaves as today, its scratch its host's.
- The Miri slate pins a header laid at `'lent` in a tenant's own scratch, read
  a step later, and the lender disposing at the tenant's release.

**Directions.**

- *Which tenants take one* — open. Every tenant holding a lend of its own, or
  any tenant the embedder elects. Recommended: the embedder's election, as
  tenant election itself is.
- *How the write home splits* — open. A write home names one cell for both
  halves; a tenant with a scratch of its own has its host's region and its
  own scratch, and so do its tenants. Either every door that resolves a write
  home (`enter`, the scratch delivery, `scratch_named`, the receipt runs, the
  tenant counts) resolves a region home and a scratch home apart, or the
  scratch bumps get a table of their own keyed by whichever cell owns one.

## Dependencies

What this removes from koan is the count a tenant's lend leaves on its host's
scratch, and the warm and cold tiers of a keyed `Shares` body outliving the
call ([type tiers](../../design/type-tiers.md#where-each-body-interns)).

**Requires:** none — a tenant's own lend stands.
