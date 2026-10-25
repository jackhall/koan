# A stamp level laid once per frame

**Problem.** A keyed call lays its settlement's request into the asker's
region on every call, hit or miss: [`seek`](../../src/program/settle.rs)
writes a `Level` ([`stamp.rs`](../../src/scope/stamp.rs)) through the asker's
writer as the birth's level, whether or not the frame it births ever births a
callable that records it in its stamp. A loop whose every hop makes a keyed
call therefore grows its host's region by one `Level` per hop: a probe of the
asker's region at each seeking call reads 492760 B
at 256 hops, about 285 B per hop above the same loop's bytes with no level
laid (419904 B). A tail loop that seeks a settlement each hop stays flat
(1032 B at 16, 64 and 256 hops).

**Acceptance criteria.**

- A frame's level is laid once, in the region the frame writes, where its
  callee's body can birth a callable — in its own statements, its blocks or
  the module bodies it runs; a keyed call whose callee's body births none
  lays no `Level`.
- A test measuring the asker's region at each keyed call finds that a loop
  making a keyed call each hop — to a callee that births no callable, or to a
  `Fresh` callee that does — lays no `Level` there per hop.
- A callable born under a settlement still records its chain's stamp, and a
  call through its stale handle replays it
  ([replay tests](../../src/dispatch/tests/replay.rs)), a `Fresh` frame's
  level and a callable born in a block included.

**Directions.**

- *When a level is laid — decided.* A bit sealed on each body shape and
  copied to its body record says whether the body can birth a callable,
  through its blocks and module bodies but not its quotes; a call whose
  callee's body has it clear lays no level. Laying at the first birth itself
  would need a slot shared by the frame's evaluation and block cells, which
  each hold their `Within` by value.
- *Where the request waits — decided.* A `Shares` callee's frame writes its
  asker's region, so the asker lays the level at the call. A `Fresh` callee's
  frame has a region of its own, which does not exist while the asker runs:
  the `Seek` carries the request by value, the drain hands it to the frame's
  first step, and the frame lays it. One function lays it on both paths. A
  birth cannot carry it by value — about 130–220 B per keyed name, a count
  that varies, in a `Copy` birth — and an asker's scratch is gone before its
  frame runs.

## Dependencies

**Requires:** none.

**Unblocks:** none.
