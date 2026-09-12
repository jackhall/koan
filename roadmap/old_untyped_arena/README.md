# Untyped arena

> **Stale as work, kept as requirements.** Written against the old runtime,
> now behind `pending_rewrite`; the items below record what the language needs
> and are retired by the [rewrite](../rewrite/README.md) as it meets them.

Converts every composite value substrate to region-allocated, borrow-carried
storage and drives regions toward `Drop`-free untyped bump arenas — the storage
model pinned in [old_design/value-substrates.md](../../old_design/value-substrates.md).
What it buys the language: one ownership regime with no per-value refcounts, a
pinning escape (a region transfer is a refcount bump and a reach mint) with a
cost-driven copy that bounds how many regions the pins retain, O(1)
deallocation-only region death, and the deletion of the runtime residence
audits in favor of compile-enforced construction doors.

Read the design doc first: it pins the whole model and its
[§ Vocabulary](../../old_design/value-substrates.md#vocabulary) defines the terms
every item here uses (region, substrate, door, brand, witness, reach, pin,
seam, Drop-free). The conversion is done: a region's value storage is its bump
and nothing else, every family in it is `Drop`-free — `Scope` included, its
freedom structural and compile-asserted — and a scope's binding tables are
bump-backed down to their keys and entry payloads. What is left is one policy
decision the shipped cost seam makes local (evacuating a dying frame). The
`ContainerSubstrate<'a, C>` shape
([src/machine/model/values/container_substrate.rs](../../src/machine/model/values/container_substrate.rs))
is the realized pattern a later conversion copies: one `Copy` wrapper over cells,
a bump-hosted index, and a stored reach the doors derive.

## Items

Every requirements doc in this retired project.

- [Region evacuation at frame death](region-evacuation.md)
