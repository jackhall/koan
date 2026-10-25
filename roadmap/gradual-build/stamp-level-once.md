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

- A frame lays its stamp's level in its region once, at its first closure
  birth; a keyed call whose frame births no callable lays no `Level`.
- A test measuring the asker's region at each keyed call finds that a loop
  making a keyed call each hop lays no `Level` per hop.
- A callable born under a settlement still records its chain's stamp, and a
  call through its stale handle replays it
  ([replay tests](../../src/dispatch/tests/replay.rs)).

**Directions.**

- *Where the request waits — open.* The `Call` birth carries the request by
  value and the frame lays it in its own region at its first closure birth;
  or the asker lays it in its scratch and the frame copies it into its region
  at that birth. Recommended: by value in the birth, if the request's size
  (the key, the binding's handles and each module's digest) stays small
  enough to move.

## Dependencies

**Requires:** none.

**Unblocks:** none.
