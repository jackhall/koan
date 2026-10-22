# The load walk's repeated work

A keyworded use's narrowing, a unit's steps and the shape table's construction,
each fact computed once.

**Problem.** The load walk and the shape table it walks recompute what they
already hold:

- **A keyworded use's candidates.** `narrow`
  ([`walk.rs`](../../src/dispatch/walk.rs)) calls `candidate(level, c)` twice
  per candidate — once in the pass gathering which slots some candidate solves
  from, once in the judging pass — and each call follows the coordinate's
  chain and reads its registration; `candidate` itself calls `surfaced`, and
  `narrow` calls `surfaced` again in the judging pass and once more for a lone
  survivor. `solving_slots(shape)` runs in the gathering pass, again in
  `solved_from` per judged candidate, and again in `instances` through
  `solved_from` and per instance argument through `solved_earlier`: three to
  four times per candidate per use. `use_exits` copies the argument types into
  a fresh scratch vector per kept candidate, and `slot_exits` collects the
  shape's slots into another.
- **A member read's operand.** The walk's member-read arm and `head` call
  `peeked(level, operand)`, then `peeked_member_scheme`, which calls `peeked`
  again; `peeked` recurses
  down a member chain, so a chain of n reads re-reads its root n times.
- **A unit's statements.** `statements(shape, work)` collects, sorts and
  dedups a component's statements in `births` and again in `steps`, and
  `steps` walks the list three times, each pass calling `nested_in` per
  statement.
- **Quadratic scans over small sets.** `guards`
  ([`unit.rs`](../../src/elaborate/unit.rs)) rebuilds its guard iterator and
  calls `typed(earlier)` for every earlier guard per guard: O(n²) store reads
  per shape. `own_levels` finds each group variable's name by a linear `find`
  over the quantifier map: O(g²) per group.
- **The shape table's construction.** The tail fixpoint `tails`
  ([`tails.rs`](../../src/scope/shapes/tails.rs)) asks `varying.contains`, a
  linear scan of a `BumpVec<(Segment, Slot)>`, per edge argument per round —
  O(|edges|·|params|·|varying|) a round, up to |varying| rounds — and again per
  parameter when it lays each body's invariants; its `rebound` pass filters
  every edge once per callable. `number_chain`
  ([`shapes.rs`](../../src/scope/shapes.rs)) dedups path identities by a
  linear `position`: O(n²) in a chain's spelled paths. `memos`
  ([`memos.rs`](../../src/scope/shapes/memos.rs)) walks every statement of a
  shape twice through `own_nodes`. `seal`
  ([`build.rs`](../../src/scope/shape/build.rs)) recounts a draft's whole
  subtree through `descendants` on every seal: O(n·d) over n bodies nested d
  deep.

**Acceptance criteria.**

- `narrow` reads each candidate's known shape, surfaced heads and solving
  slots once per use, and `instances`, `solved_from` and `solved_earlier`
  read those values rather than recomputing them.
- `use_exits` and `slot_exits` allocate nothing per kept candidate beyond what
  the use allocates once.
- A member read's operand is peeked once per read.
- A unit's statements are collected once per unit and walked once by `steps`.
- `guards` types each type expression at most once per shape, and
  `own_levels` reads each group variable's name in constant time.
- The tail fixpoint tests and sets a parameter's varying bit in constant time,
  and the `rebound` pass reads each edge once overall.
- `number_chain` numbers a path identity in expected constant time, `memos`
  walks each statement once, and sealing a body counts its descendants in
  time linear in the program.
- A load test over a keyworded use with many candidates and a deep member
  chain counts calls to `candidate`, `solving_slots` and `peeked`, and reads
  one per candidate, one per candidate and one per read respectively.

**Directions.**

- *Where a candidate's facts live — open.* A scratch slice built at the top
  of `narrow`, one entry per candidate, handed to `instances`, `solved_from`
  and `solved_earlier`; or a memo on the pass keyed by level and candidate,
  which a candidate read from several uses at one level shares. Recommended:
  the scratch slice, since nothing outside one use reads it.
- *The varying set — open.* One `BumpVec<bool>` per body sized by its slots,
  indexed by segment; or one flat bit vector over every body's slots, offset
  per segment. Recommended: the flat vector, one allocation per load.
- *`guards` — open.* Type every guard once into a scratch vector and compare
  handles by a sort on (arm set, handle); or keep the pairwise scan over the
  typed vector, O(n²) comparisons but O(n) store reads. Recommended: the
  sort, which reports the first repeat in program order after a stable sort.

## Dependencies

**Requires:** none.

**Unblocks:** none.
