# Typed pins for the pinned fold verbs

**Problem.** The pinned fold verbs — `Witnessed::map_pinned` /
`map_pinned_placing` / `merge_pinned` / `merge_pinned_placing`
([witnessed.rs](../../workgraph/src/witnessed.rs)) — accept `pin: &Pin` with
only `Pin: Witness`: no type links the pin to the operand backing it must keep
alive across the transient re-anchor. Every call site discharges the
obligation in prose ("The pin: the destination frame, whose arena holds …") —
including a possibly-empty `FrameSet` pin in `build_type_operand`
([constructors.rs](../../src/machine/execute/dispatch/constructors.rs)),
covered in that arm by the live `scope` borrow rather than by the pin passed.
The destination operand's backing across `merge_composed` is likewise
call-site prose ("covered by its own live destination, which the caller
necessarily holds"). A call passing an unrelated witness as `pin` re-anchors
over unpinned backing — dangle-capable safe code.
`Delivered::transfer_into` / `transfer_into_placing`
([delivered.rs](../../workgraph/src/witnessed/delivered.rs)) are the typed
precedent: the engine pins with its own bundled host, and the caller supplies
nothing.

**Acceptance criteria.**

- Every pinned re-anchor's pin is tied to the backing it covers by type (a
  bound or a bundled value): a call passing a witness unrelated to the operand
  is a compile error.
- The destination operand's backing coverage across a pinned merge is typed or
  engine-derived, not a call-site comment.
- The "The pin: …" justification comments at koan fold call sites are gone —
  the signature carries the obligation.

**Directions.**

- Link mechanism — open. (a) Bundle the pin with the operand as `Delivered`
  does (carriers travel with their hosts; the verbs take no pin parameter);
  (b) an `unsafe` marker trait tying pin type to operand family, implemented
  per honest pair; (c) derive the pin from the operand head's region owner.
  Recommended: (a) — the shape already exists as `Delivered`, and it removes
  the parameter rather than constraining it.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none.
