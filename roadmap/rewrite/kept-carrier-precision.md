# Precise kept carriers

**Optional.** No profile asks for it yet; each candidate below lands when a
measurement does.

**Problem.** A tail loop hands a value on from where it rests only where the
frame keeps a carrier of it at its true home
([what a loop keeps](../../src/program/README.md#frames-contracts-and-tails)).
Three kinds of value have no such carrier, so a loop copies them at every hop
that hands them on:

- A capture, a top-level name or a module member passed on a tail call is
  copied into the loop's cell each hop it is passed: O(its size) per hop.
- A part of its argument that a spawned call hands back. A spawned frame's kept
  carriers are projected from its birth, so they are homed where its caller
  built the argument record — the calling hop's own frame. A helper that
  returns a part of its argument hands back a value homed in the hop, and a
  loop that threads it copies it. A walk written
  `(LET n = (ONWARD link)) (LOOP m n)`, where `ONWARD` returns
  `ATTR link next`, copies the rest of the chain every hop — quadratic in the
  chain's length — where `(LOOP m (ATTR link next))` copies nothing
  ([`a_call_returning_a_part_of_its_argument_places_nothing_new`](../../src/dispatch/tests/loops.rs)).
- The callee of a hop into another function. A frame keeps a carrier of its
  own callee, so a hop into itself hands it on free, but a mutual hop
  (`PING` into `PONG`) reads its callee by name and has none: the callee's knot
  is copied into each hop's birth, some 3 KB a hop for the mutual loops in
  [`loops.rs`](../../src/dispatch/tests/loops.rs).

**Acceptance criteria.**

- A tail loop that passes a capture, a top-level name or a module member on
  unchanged copies a constant number of bytes per hop, independent of that
  value's size.
- A mutual tail loop copies none of its callees per hop: the bytes each hop
  copies are the same as for the loop hopping into itself.
- A chain walk through a helper that returns a field of its argument copies no
  part of the chain per hop, and holds every region flat in its depth, as the
  walk that reads the field itself does.

**Directions.**

- *A capture's kept carrier — open.* A capture is a part of its callee's
  closure, and a frame already keeps its callee at its true home. Recommended:
  project a capture's carrier out of the frame's kept callee.
- *A mutual hop's callee — open.* A top-level function or a module member
  rests in the run root or its module, above every hop, but a name read of it
  carries no kept carrier. Candidates: a kept carrier read beside a name that
  lands on a top-level or module binding, as a frame's own slots are; or a
  skeleton naming such a callee by its binding, so the successor reads it where
  it rests.
- *A spawned call's arguments — open.* Build every call's birth from one
  operand per argument, as a hop's successor builds its own, so each argument
  keeps the carrier of its true home; or have the frame's first step re-home
  each projected carrier where its argument's bytes rest. Recommended: the
  first, which reuses the hop's door.

## Dependencies

**Requires:** none.

**Unblocks:** none.
