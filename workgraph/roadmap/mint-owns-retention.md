# Mint owns its retention

Fold the retention decision into the mint, so workgraph owns every reach
mint and an embedder's only reach input is the copy-or-pin verdict.

**Problem.** Retention is a separate call a caller makes *after* a mint.
[`ReachDescription::mint`](../src/witnessed/reach.rs) returns
the owned bundle and leaves its home to the caller:
`RegionHandle::mint_retained`, `Carrier::compose_into`,
`Delivered::open_adopted` and `Sectioned::build` fold it into the
destination region by hand, while `Carrier::mint_into` and
`Carrier::resident_in` hand it out to a holder that owns the pins itself.
The mint cannot tell the two apart, so it cannot record which regions it
made the destination pin — and the region carries a
`retained_descriptions` address set
([`Region`](../src/witnessed/region.rs)) purely to recover
that fact when a later mint interns a hit. `RegionHandle::retain_reach` is
public besides, so an embedder folds reach into a region directly
([`Scope::retain_reach`](../../src/machine/core/scope/reach.rs),
[`interpret.rs`](../../src/machine/execute/runtime/interpret.rs)).

**Acceptance criteria.**

- Every mint names its destination role — the value rests in the
  destination region, or it travels on under a holder's own pins — and the
  mint performs the matching retention itself; no caller folds a minted
  bundle by hand.
- The region's `retained` address side table is deleted: an intern miss
  performs the retention, and an intern hit is proof the destination
  already pins the entry's members.
- `RegionHandle::retain_reach` is off the embedder surface — an embedder
  supplies copy-or-pin verdicts and born-borrowing seeds, and has no
  vocabulary for folding reach into a region.
- The Miri audit slate is green.

**Directions.**

- *How the role is named — open.* (a) a destination-role argument on the
  one `mint`; (b) two doors, one per role, whose return shapes differ — a
  resident mint yields the description alone, an in-transit mint yields
  description plus bundle. Recommended: (b), which makes a caller that
  wants the pins unable to reach the resident door at all.
- *Koan's direct retentions — open.* Whether `Scope::retain_reach` and the
  run-teardown rehome route through a role-named door or disappear into
  the sectioned alloc door; settles once
  [Sectioned substrates](../../roadmap/untyped_arena/sectioned-substrates.md) has
  moved koan's substrates onto stored reach.

## Dependencies

**Requires:** none — the interned side table it builds on is shipped
([workgraph/design/sectioned-reach.md](../design/sectioned-reach.md)).

**Unblocks:**

- [Carving the cellgraph crate](cellgraph-extraction.md) — dropping
  `retain_reach` from the embedder surface before the carve keeps it final.
