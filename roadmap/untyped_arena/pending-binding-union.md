# Pending bindings as a value state

Concerns the binding tables of
[src/machine/core/bindings.rs](../../src/machine/core/bindings.rs); the
scheduler's park-and-replay behavior they serve is described in
[design/execution/README.md](../../design/execution/README.md).

**Problem.** A parked bind lives in a side table (`placeholders`,
`pending_overloads`) parallel to the table it will resolve into (`types`,
`data`, `functions`). The exclusivity invariant — a name is never pending and
bound at once — is cross-map discipline rather than a type; a lookup probes the
side table and then the real one; and finalization is a remove from one map
plus an insert into another, storing the key twice. Once the tables are
bump-backed, that cross-container move also abandons the pending entry's bytes,
where an in-place overwrite would abandon nothing.

**Acceptance criteria.**

- `placeholders` and `pending_overloads` are deleted as tables; each
  destination table's entry grows a pending arm — a `{Bound, Pending}` union
  for `types` / `data` values, a pending variant beside the sealed entries in a
  `functions` bucket. A pending overload already keys on the same full
  `UntypedKey` as the overload it becomes, so it lands in the bucket it
  resolves into.
- Resolution overwrites the pending arm in place — no remove-plus-insert, the
  key stored once.
- A name lookup probes one table; parking on a pending arm replays exactly as
  parking on a placeholder does — observable scheduling behavior unchanged.
- The producer-failure sweep (the retain-by-`NodeId` purge) filters pending
  arms with unchanged observable behavior.

## Dependencies

**Requires:**


**Unblocks:**

- [Bump-backed binding tables](bump-backed-bindings.md)
