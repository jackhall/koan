# Record field types in type position

Naming a record field's declared type as `:(Point.y)`.

**Problem.** The elaborator's `Attribute` arm
([elaborate/expression.rs](../../src/elaborate/expression.rs)) reads only a
union's member, through `union_member_named`, so `:(Point.y)` — the declared type
of `Point`'s field `y` — is `NoSuchMember`, and a slot typed by a field's
declared type (tutorial 08's `LABEL`) cannot be written.

**Acceptance criteria.**

- `:(T.field)` in type position denotes the declared type of `field` in the
  record type `T` is, and works through an alias of `T`.
- The read chains where a field is itself record-shaped: `:(Outer.inner.x)`.
- A field `T` does not declare is an elaboration error naming `T` and the
  field.

**Directions.**

- *Through a newtype layer — open.* A field read falls through every newtype
  layer to the record in [dispatch](dispatch.md); whether `:(Boxed.x)` reads
  through `NEWTYPE Boxed = Point` too is undecided. Recommended: yes, so a
  field's value and its declared type are reached the same way.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Dispatch](dispatch.md) — tutorial 08's `LABEL` snippet.
