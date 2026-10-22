# Fact reads at their cost

The load's records read in one lookup, by reference, and indexed only where
the shape cannot answer.

**Problem.** Reading a fact the load recorded costs more lookups, copies and
bytes than the fact needs:

- **The records index.** `Records.index`
  ([`records.rs`](../../src/scope/records.rs)) holds a `(Held, u32)` entry,
  24 bytes, for every fact of every unit — beside facts of 8 to 40 bytes — and
  `of_records` rebuilds it with a full sort of every key. It keys
  `Held::Slot`, `Held::Statement` and `Held::Born`, whose unit the shape
  already answers through `unit_of_slot`, `unit_of_statement` and
  `Shapes::parent`, so "which unit holds this" has two mechanisms.
- **A path read by the evaluator.** `member_type` and `inline_member` on
  `ActivationView` ([`reads.rs`](../../src/elaborate/reads.rs)) and
  `solutions` ([`typed.rs`](../../src/scope/typed.rs)) read one path through
  `path_entry` and `path_closed` (via `resolution`), two `records.path`
  lookups, each a probe of the index and of the holding unit's `paths`, where
  one returns both halves.
- **Off-chain facts by value.** `inline_facts`, `owner_facts` and
  `off_chain_slot` ([`walk/reads.rs`](../../src/dispatch/walk/reads.rs))
  return a whole `Typed` record by value — a slice per fact table — to read
  one table.
- **An encloser's gathering.** `gather` on an open unit that is not the
  innermost copies its whole gathering into a scratch vector and `note`s it,
  binary-inserting each element into the innermost, each insert shifting the
  tail: O(n log n) comparisons and up to O(n²) moves per read of an
  encloser's fact. The gathering is kept
  sorted by those inserts, and `end` ([`walk.rs`](../../src/dispatch/walk.rs))
  sorts and dedups it again through `Frontier::laid`.
- **A chain's path table.** `table()`
  ([`walk/paths.rs`](../../src/dispatch/walk/paths.rs)) walks the chain back
  to its root through `chain_root` and `rposition`s the open tables on every
  call; `resolve_path`, `through_paths`, `is_path`, `reachable` and
  `locatable` each call it, and `entry_at` and `resolve_spelled` call it
  twice each.

**Acceptance criteria.**

- `Records.index` holds only the keys the shape cannot answer, and a fact
  under any other key is found through the shape's unit lookup and the unit's
  record.
- A path read by the evaluator costs one records lookup.
- Reading one table of an off-chain body's facts copies no `Typed`.
- Reading an encloser's fact gathers its unknowns without copying the
  encloser's gathering, and a unit's frontier is sorted once.
- A chain's path table is found in constant time from its level.
- A load test over a program of many units reports the index's byte size
  beside the facts', and the index is no larger than the site, path and
  through facts it keys.

**Directions.**

- *The index's key set — open.* Drop `Held::Slot`, `Held::Statement` and
  `Held::Born` and answer them through the shape's unit lookup and
  `units.get(unit)`; or keep every key but compress an entry to a `u32` key
  offset beside the unit index. Recommended: dropping the keys, which also
  removes the second "which unit" mechanism.
- *Facts by reference — open.* `inline_facts` returns `&Typed`, borrowing the
  pass for a running unit's scratch and `'o` for a written record; or it takes
  a projection closure, as `LoadFacts::fact` does, and returns the one table;
  or a running unit lays its inline bodies' facts into `'o` as each body
  leaves the chain, so every read is `&'o`. Recommended: the projection.
- *The gathering — open.* `note` pushes unsorted and `end` sorts and dedups
  once, at the cost of duplicates held until then; or `note` keeps the
  innermost sorted and `Frontier::laid` takes the sorted vector as is, with a
  merge in place of the copy for an encloser's gathering. The doors that
  mutate it through `Cell` replace-and-set are logically `&mut self`, which
  either choice may take up. Recommended: sort once at `end`.
- *The path table's index — open.* Cache it on each `Level` as the level is
  pushed; or keep it on the chain's root level and read it through
  `chain_root`. Recommended: on each level.

## Dependencies

**Requires:** none.

**Unblocks:** none.
