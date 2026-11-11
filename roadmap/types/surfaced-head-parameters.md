# A surfaced head reads only the parameters its shape names

**Problem.** A `USING … SCOPE` block's surfaced head is exactly its declared
return under the call's solve only where its registered shape is closed
([static types](../../src/dispatch/README.md#static-types)). The head's entry
binds every head parameter of its signature, including one its shape never
names (`head_entry` in [`unit.rs`](../../src/elaborate/unit.rs)), and an
unpinned one is read at the block's own type name, which a run binds, so any
such read makes the shape rigid. Under the `Viewed` and `m` of the dispatch
test `a_quantified_keyworded_member_returns_what_its_declared_shape_returns`,
whose `GET` names no `Carrier`, a header or view leaving `Carrier` unpinned —
`h = (m :! Viewed)` or `h = (m :| Viewed)` — makes the call at most the head's
return, and `USING h SCOPE (ATTR (GET 1) y)` loads and faults `has no field y`
at run. Over `m :! (Viewed WITH {Carrier = Number})` or a signature with no head
parameter, the same read refuses the load.

**Acceptance criteria.**

- A surfaced head whose shape names no unpinned head parameter is closed:
  `USING h SCOPE (ATTR (GET 1) y)` refuses the load with
  `:{x :Number} has no field y` over `h = (m :! Viewed)` and over
  `h = (m :| Viewed)`.
- A head whose shape names an unpinned head parameter is read as it is today.

**Directions.**

- *Which parameters a head's entry reads — decided.* Only those its shape
  names, so a parameter it never reads leaves no rigid read behind.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
