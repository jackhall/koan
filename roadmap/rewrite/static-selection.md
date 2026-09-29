# Static selection

A keyworded call's candidates narrowed, and where possible chosen, where the
shape is built.

**Problem.** No binder and no value expression has a type before the program
runs, so each keyworded call
[admits its candidates afresh](../../src/dispatch/README.md#selection) against
its arguments' carried types, and a call no candidate can ever admit is found
only when it runs, on the path that reaches it.

**Acceptance criteria.**

- Every value expression and binder has a static type where the shape is built,
  and on every run the value's carried type lies under it; one whose type the
  load cannot bound has `Any`.
- Where the shape is built, a keyworded use's candidate list drops every
  candidate whose slots meet its arguments' static types at `Never`, a rigid
  variable read through its bound, and a use at a key with no such candidate
  refuses the load, located at `path:line:col`.
- A use left with one candidate that admits its arguments' static types selects
  it where the shape is built, and the call runs it without admitting its
  arguments; a quantified candidate still solves its group from the carried
  types.
- On every call, the candidate a narrowed or static selection runs is the one
  selection over the full list by the carried types would run.

**Directions.**

- *Where static types come from — decided.* The
  [type channel's load pass](../../src/elaborate/README.md#the-type-channel-at-load), extended to the value
  channel: local and bidirectional, a declared parameter or return flowing down,
  a literal's, a construction's and a selected callee's return flowing up.
- *Narrow, then select — decided.* Narrowing drops only a candidate that can
  never admit a value under the static types, so it runs everywhere; selection
  happens where one candidate is left and it admits them outright.
- *A use no candidate can admit — decided.* Refused at load, as a statically
  typed language refuses it.
- *Where a narrowing rests — open.* Recommended: a write-once cell beside the
  use's candidate list on the shape, as the type channel's handles rest.
- *`MATCH` arms — open.* An arm set whose scrutinee has a static type could narrow
  its guards the same way.

## Dependencies

**Requires:** none.
