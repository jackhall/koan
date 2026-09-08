# Retire the record vocabulary

Rename the crate's second tier from **record** to **sealed cell**, so a word
koan already spends on labelled-field values does not also name a retained
region ([design/liveness-matrix.md § The sealed tier](../design/liveness-matrix.md)).

**Problem.** `record` is koan's word for a labelled-field value — record
types, `FROM` record projection, the `:{…}` record repr of a `NEWTYPE`. The
substrate spends the same word on something with no relation to it: a region
whose cell died while something still reached its storage. Nothing about the
two is analogous, and the collision lands the moment koan adopts the crate,
in exactly the code where both readings are live at once.

The vocabulary is not a handful of sites. `record` appears on 424 lines under
`cellgraph/src`, 58 under `cellgraph/design`, and 14 across the two READMEs —
`SealedRecord`, `record_bytes`, `retire_record`, `reclaim_record`,
`fold_into_record`, `relocate_to_record`, `record_count`, `seen_records`,
`Occupancy::records`, `SlabForward::Record`, `TransitivePins::records`, and
the prose that carries every one of them.

The rename also raises what `CellRef` should be called once "cell" covers
both tiers. It is the sum of `Handle` and `TreeHandle` — the two kinds that
occupy a *place* and therefore carry a generation. A sealed cell carries none:
its id is drawn once and never re-bound, so there is nothing for a handle to
re-check. That, not liveness, is the line the type draws, and the name has to
draw the same one — `CellRef` is handed out stale (`Stale<CellRef>`), and
`ResidentKey::home` holds one for a cell that may since have departed.

**Acceptance criteria.**

- No identifier or prose under `cellgraph/src`, `cellgraph/design`,
  `cellgraph/README.md`, or `cellgraph/roadmap/` uses `record` for a sealed
  region. `SealedRecord` is `SealedCell`; every derived name follows it.
- `record` survives in the crate only where it means what koan means, or in
  its ordinary English sense (`recorded`, `recording`, a figure "on record").
- `CellRef` is renamed to say what separates it from a sealed cell — that its
  two variants name a place and carry a generation — and still has exactly two
  variants. The name must stay true of a stale one and of a departed
  `ResidentKey::home`.
- The `SealedTier`'s own naming is settled in the same pass rather than left
  half-renamed — either it keeps its name over a tier of sealed cells, or it
  moves with the rest.
- `tools/verify.sh` is green and `tools/doclinks.py check` reports no broken
  link, including the `#`-anchors into the design tree that section titles
  carrying the old word currently answer.

**Directions.**

- *What `CellRef` becomes — open.* `LiveCellHandle` overclaims: the type is
  handed out stale and a `ResidentKey`'s home may have departed. `CellHandle`
  is true but says nothing about the split. Something naming the generation —
  or the place — would say it, at the cost of a longer name on a public type.
- *Whether `CellRef` grows a sealed case — closed, record why.* The rename
  makes "sealed cell" a cell by name, which invites a third variant. Nothing
  in the substrate wants one: an embedder never holds a sealed id, and the
  tier skips generations precisely because a sealed name cannot be re-bound.
- *Order of operations — open.* Either one mechanical sweep with the prose
  swept behind it, or design-tree first so the sentences that carry the
  concept are settled before the identifiers follow them. The second is
  slower and leaves a half-renamed tree between commits.

## Dependencies

**Requires:** none — the sealed tier is shipped and no consumer names it yet,
which is the whole reason to do this before koan adopts the crate.

**Unblocks:**

- [Settle the public surface's vocabulary](public-surface-vocabulary.md) — the
  sibling rename pass, which waits on the `CellRef` ruling this item takes.
