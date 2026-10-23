# Shared frontiers

Each unit's frontier stored once, however many units read it.

**Problem.** A unit's frontier is its gathering at `end`
([`walk.rs`](../../src/dispatch/walk.rs)), sorted and deduplicated once there
([`Frontier::laid`](../../src/scope/facts.rs)): every unknown it depends on,
transitively ([the store](../../src/build/README.md#the-store),
[frontiers and seams](../../src/build/README.md#frontiers-and-seams)). A read
of an ended unit's fact copies that unit's whole frontier into the innermost
gathering (`gather` → `note`, [`reads.rs`](../../src/dispatch/walk/reads.rs)).
A unit that reads one frontier several times copies it each time, and the
duplicates go only at `end`. Every consumer reads a frontier flat: a wake
merges it against the binding (`Frontier::meets_binding`, behind each
`Store::wakes`: [`load.rs`](../../src/build/load.rs) and the test stores in
[`stores.rs`](../../src/program/tests/stores.rs)), and the folds
([`folds.rs`](../../src/build/folds.rs), `keyed_names`) read the latest bind
site among its unknowns, then walk each unknown for the names a key binds. No
two frontiers share structure:

- **A chain of reads.** Over a chain of k statements each reading the one
  before (`LET v_i = (v_{i-1} :| Counter)`), unit i's frontier holds the ~i
  binders before it, so the frontiers laid total ~k²/2 entries.
- **Repeated gathers.** Each unit gathers earlier frontiers about six times
  (6,016 gathers over 1,009 units at k = 1000), copying each in full.
- **What it costs.** An instrumented release build over a closed view chain
  at k = 250, 500 and 1000 lays 32k, 127k and 505k frontier entries, notes
  160k, 633k and 2.5M unknowns, and grows the walk's scratch arena to 8.4,
  33.6 and 134 MB. The load's max RSS grows about 4× per doubling of k: 13,
  28 and 78 MB. A chain of open views, over a parameter, lays the same
  frontier entries and notes about twice its own entries (1.0M at k = 1000),
  where the closed chain notes about five times its own.

**Acceptance criteria.**

- The frontiers of a chain of k units, each reading the one before, take O(k)
  storage in all.
- Gathering an ended unit's frontier costs O(1) per read, however many
  unknowns that frontier holds.
- For every record, a wake (`Frontier::meets_binding`) and each fold — a
  unit's seam and the names its key binds — give the answer they give over
  the record's flattened frontier, and the resumption and build suites pass
  unchanged.
- A load test over such a chain, through `load_counts` and `assert_linear`
  ([`tests.rs`](../../src/dispatch/tests.rs)), counts frontier entries laid
  and unknowns noted, and each is linear in k.

**Directions.**

- *Representation — open.* A unit's frontier is its own unknowns beside edges
  to the units whose frontiers it read — a DAG at unit grain — flattened and
  memoized per binding when a wake or a fold asks; or frontiers are persistent
  sorted sets that share structure, each the frontier it read plus a delta.
  Recommended: the DAG, since a read then records one edge.
- *Seams over a shared frontier — open.* A unit's seam is the latest of its
  own unknowns' bind sites and its edges' seams, memoized per unit; or the
  frontier is flattened before a fold reads it. Recommended: the per-unit
  memo.

## Dependencies

[The build arena](build-arena.md) keys build cells on frontier classes, so
this item best lands before it or with it.

**Requires:** none.

**Unblocks:** none.
