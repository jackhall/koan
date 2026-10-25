# Flat memory for a hop under its own settlement

**Problem.** A tail loop whose every hop stays under its frame's own
settlement — a keyed callee hopping into itself down a chain of links, as
`RUNS` in the [tail tests](../../src/dispatch/tests/tail.rs) does through
`tail_under` ([`evaluate.rs`](../../src/dispatch/evaluate.rs)) — grows its
host's region by about 600 B per hop: a probe of the region reads 157152 B at
256 hops. A tail loop that seeks a settlement each hop stays flat (1032 B at
16, 64 and 256 hops). Which bytes each hop leaves has not been diagnosed.

**Acceptance criteria.**

- A test measuring the host's region at 16, 64 and 256 hops of a tail loop
  hopping under its own settlement finds it flat in the depth.
- The hop still keeps its frame's settlement and level: the tail tests'
  count of hops under a frame's own settlement is unchanged.

**Directions.**

- *Cause — open.* The hop may be placed `Shares`, laying the next frame's
  activation and argument record in its host, which reclaims nothing until
  the host finishes; or the hop's own bookkeeping (its activation's records,
  its `Within`) may be laid per hop. A per-hop byte count by allocation site
  decides which before any fix.

## Dependencies

The `Shares` case is the rewrite project's unplanned
[`Shares` tail hop](../rewrite/README.md#unplanned-work) in one setting.

**Requires:** none.

**Unblocks:** none.
