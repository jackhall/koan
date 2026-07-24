# Residence-audit retirement

**Problem.** The runtime residence audit — the
[`Residence`](../../src/machine/core/arena/residence.rs) ownership predicate
and the evidence-tier move-in doors layered on it — vets every composite
move-in at runtime against reach evidence the scopes mint and thread by hand.
It is a runtime-audited enforcement tier: once the envelope-host/residence
pairing is typed and pin liveness is holder-owned, each audit site re-checks
what the carrier types already guarantee, but no per-site disposition exists —
every site still runs, and the tier reads as enforcement rather than the
stopgap it is.

**Acceptance criteria.**

- Every runtime residence-audit site is dispositioned: deleted where the typed
  host/residence pairing subsumes it, or explicitly documented as a redundant
  backstop in [design/witness-hosting.md](../../design/witness-hosting.md).
- The Miri audit slate is green after the retirements.

**Directions.**

- *Retire versus backstop — open.* Delete a subsumed site outright versus
  keeping a debug-build backstop;
  [drop-free region death](drop-free-region-death.md) deletes the composite
  tiers regardless, so any kept backstop is transitional.

## Dependencies

**Requires:**

- [Reach ownership split](reach-split.md) — holders own their pin bundles, so
  pin liveness is typed rather than audit-maintained.

**Unblocks:**

- [Drop-free region death](drop-free-region-death.md) — its deletion of the
  composite residence tiers builds on the per-site disposition here.
