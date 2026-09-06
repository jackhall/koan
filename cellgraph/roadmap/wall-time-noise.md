# Wall-time noise in the perf sweep

**Problem.** Two builds of identical source disagree about how long a verb
takes, by more than the changes the crate's work items make. `python3
tools/cellgraph_perf.py --calibrate` sweeps HEAD against a worktree rebuild of
HEAD — same source, two binaries, alternated so a machine that drifts through
the sweep drifts through both, fastest of fifteen trials each — and still reads
16.1 % on the worst row above 20 µs with the next at 9.5 %, and far more below:
`keep_redeem/16` reads 46 % on one calibration and 65 % on another, and
`push_chain/8` 10–16 %. The spread is stable within a sweep and moves between
builds, which points at code layout fixed at the link rather than at machine
load, and it lands on the same rows every time —
`push_chain/8/{alloc_into,release,enter}`, `push_chain/32/alloc_into` and
`keep_redeem/16/*`, the small-`n` rows with the fewest calls behind a reading.

The time gate in [tools/cellgraph_perf.py](../../tools/cellgraph_perf.py) is
set above that floor and pays for it twice. `TIME_TOLERANCE` is 20 %, so a
regression that costs a verb a fifth of its time passes unremarked.
`TIME_FLOOR_NANOS` is 20 000, and 81 of the 88 rows in
[observe/perf.csv](../observe/perf.csv) read below it, so every benchmark the
crate is measured on except the widest seven is ungated in wall time. A gate
set from the machine rather than from the code is a gate that reports on the
linker, which is why the tolerance was measured rather than guessed; measuring
it is not the same as removing what it covers.

**Acceptance criteria.**

- `--calibrate` reads every row's spread within `TIME_TOLERANCE` on two
  consecutive invocations against the same commit, and the tolerance is at
  most 10 %.
- `--gate-time` weighs every row of the record: no reading is so small that a
  percentage of it is timer granularity, so the gate covers the small-`n` rows
  rather than the seven widest.
- A `--gate-time` sweep of a commit against a rebuild of that same commit marks
  no row slower, and two such sweeps agree on the rows they mark.
- A `--gate-time` sweep names the bar each row was held to, so which rows the
  gate weighed and which it excluded is readable off the run rather than off
  the tool's source.

**Directions.**

- *Where the spread comes from — open.* (a) Code layout fixed at the link, the
  leading hypothesis: the spread is stable within a sweep and moves between
  builds of identical source. (b) Allocator state — two processes with
  different heap histories reach a verb with differently shaped free lists.
  (c) The harness's own per-row setup, which at a small `n` is a larger share
  of a reading than the verb it brackets. Attributing the spread comes first;
  the remaining bullets are chosen against what it names, and picking one
  before the reading is guessing again.
- *Averaging the layout out — open.* Raise the harness's per-row call count
  until a reading covers enough of the binary for layout to cancel. Cheapest to
  try and costs only sweep time, but it may not converge: a hot loop that
  straddles a cache line straddles it on every call.
- *A measured threshold — open.* A third arm — the baseline built twice, all
  three run alternating — so each row's own build-to-build spread is read in
  the same sweep and the gate's bar is that figure rather than a constant. Turns
  the tolerance into a reading; costs a third build and a third of the trials.
- *The floor — open.* Whether a row too small to time is excluded at all, or
  whether the harness repeats such a row until it is large enough to read.
  Recommended: the latter, since an excluded row is an ungated verb and the
  exclusion is what leaves 81 of 88 rows unwatched today.

## Dependencies

**Requires:** none — the readings come from
[tools/cellgraph_perf.py](../../tools/cellgraph_perf.py) against the record in
[observe/perf.csv](../observe/perf.csv).

**Unblocks:** none — leaf.
