# Wall-time noise in the perf sweep

**Problem.** Two builds of identical source disagree about how long a verb
takes, by more than the changes the crate's work items make. `python3
tools/cellgraph_perf.py --calibrate` sweeps HEAD against a worktree rebuild of
HEAD — same source, two binaries, alternated so a machine that drifts through
the sweep drifts through both, fastest of fifteen trials each — and still reads
16.1 % on the worst row above 20 µs with the next at 9.5 %, and far more below:
`keep_redeem/16` reads 46 % on one calibration and 65 % on another, and
`push_chain/8` 10–16 %. The spread is stable within a sweep and moves between
builds, and it lands on the same rows every time —
`push_chain/8/{alloc_into,release,enter}`, `push_chain/32/alloc_into` and
`keep_redeem/16/*` — which are the rows of the first shapes each process runs,
and the ones with the fewest calls behind a reading.

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

- *Where the spread comes from — decided.* Measured, not the link: two builds'
  loaded images are byte-identical, and a byte copy of one binary spreads as
  much against it as the rebuild does. First-order is cold start — every shape
  runs once per process in a fixed order, so the first shapes read the
  process's cache and heap first-touch as the verb's cost (`keep_redeem/16/create`
  1303 ns cold, 160 ns after one warm-up pass, 70 ns as the fastest of twenty
  in-process repeats); a clock-ramp spin alone does not remove it. Second-order,
  once warm, is the physical placement of the file's own pages: two files with
  identical bytes hold a stable 4–12 % offset on a few rows that survives
  ASLR-off, environment and path changes, and forced alignment, and is redrawn
  by copying the file. Meter overhead is 96 ns per measured call, so a one-call
  row is the meter on both sides. Findings and figures in
  [scratch/wall-time-noise.md](../../scratch/wall-time-noise.md).
- *Averaging the layout out — decided.* Repeat each shape in-process: grow a
  block of runs until the smallest row's block total clears the floor (the
  undersized blocks are the warm-up), run several blocks, keep each row's
  fastest block, and report it per run as an integer, so `nanos` keeps its
  meaning and the record its schema — the smallest rows quantize at ~1 ns in
  76, which the tolerance covers. Measured: every row clears the floor by
  construction, a process run costs 0.45 s, and the spread against a fixed pair
  of files falls to a median of 1.5 % with the file-placement offset as the
  residual.
- *Sampling the placement — decided.* Exec each trial from a fresh copy of the
  binary, so both sides draw their page placement anew every trial rather than
  once per build. Measured with the blocks above: every row within 5 % on two
  consecutive runs against a pair that read 12 % apart with fixed pages, median
  0.4 %. Chosen over a third build arm — the baseline built twice, all three
  run alternating — which would read one more fixed draw of the placement
  rather than average it.
- *The floor and the tolerance — decided.* With in-process blocks the floor
  sizes the block rather than excluding rows, so the gate weighs every row:
  20 µs, and `TIME_TOLERANCE` 10 %, twice the worst row the protocol read.

## Dependencies

**Requires:** none — the readings come from
[tools/cellgraph_perf.py](../../tools/cellgraph_perf.py) against the record in
[observe/perf.csv](../observe/perf.csv).

**Unblocks:** none — leaf.
