# Field reads on anonymous record values

**Problem.** A record literal stands on its own as an anonymous record value —
it binds, prints with its fields, and satisfies a `:{...}` schema slot under
width subtyping ([tutorial/07-records.md](../../tutorial/07-records.md)). It
cannot be *read*. `person.name` and its computed spelling `ATTR person "name"`
both fail on one, where the same read off a `NEWTYPE` record succeeds.
[`wrapped_field_cell`](../../src/builtins/attr.rs) reaches a record only through
a `KObject::Wrapped` layer — it matches `Wrapped { inner: Record(..) }` and
recurses through other payloads, so a bare `KObject::Record` falls to the
catch-all arm. The value is therefore second-class: constructible, dispatchable
and printable, but not projectable, and the asymmetry is undocumented —
[tutorial/reference.md](../../tutorial/reference.md) scopes `<record>.<field>` to
a `NEWTYPE` record without saying the anonymous form has no read at all.

The diagnostic compounds it. The catch-all arm reports
`type mismatch for argument 's': expected a value with fields, got :{name :Str
age :Number}` — it names the builtin's internal slot rather than the receiver the
writer wrote, and it renders the operand as its *type*, which is a schema that
manifestly has fields. A reader is told the value lacks what the message itself
prints.

The gap reaches the error surface too. A `TRY` error arm and a `MATCH … OVER
KError` arm bind `it` to the caught kind's payload record (ruling F3), which is a
bare record, so `it.name` / `it.message` / `it.frames` do not resolve —
[design/error-handling.md](../../design/error-handling.md) and
[tutorial/09-errors.md](../../tutorial/09-errors.md) both have to say so, and five
behavioural tests are `#[ignore]`d on it.

**Acceptance criteria.**

- `person.name` reads the field off a value bound from a record literal, with no
  `NEWTYPE` declaration anywhere in the program.
- `ATTR person "name"` and `ATTR person (name)` read the same field off the same
  value, so the computed spellings track the dotted one.
- A field read naming a field the anonymous record does not carry reports the
  missing field by name, the same shape a `NEWTYPE` record's miss reports.
- The read composes with the schema-typed parameter path: a body whose parameter
  is typed `:{name :Str}` reads `r.name` off an anonymous record passed to it.
- A projection binds and reads: `LET view = ((x y) FROM both)` followed by
  `view.x` yields the field.
- No field-read diagnostic names a builtin slot (`s`) where the receiver is what
  the writer wrote.
- A `TRY` error arm and a `MATCH … OVER KError` arm read their kind's fields off
  `it` — the arm binds the payload record (ruling F3), so `it.name`, `it.message`
  and `it.frames` resolve through the same bare-record read. The behavioural tests
  ignored on that binding are re-enabled: `unbound_name_arm_catches_unbound_name`,
  `shape_error_arm_catches_shape_error`,
  `type_mismatch_arm_catches_record_newtype_value_mismatch` and
  `frames_non_empty_after_recursive_call` in
  [`try_with/tests.rs`](../../src/builtins/try_with/tests.rs), and
  `failure_wraps_lowered_error_in_error` in
  [`catch/tests.rs`](../../src/builtins/catch/tests.rs).

**Directions.**

- *Where the arm goes — decided.* `wrapped_field_cell` in
  [`attr.rs`](../../src/builtins/attr.rs) is the single chokepoint both `.` and
  `ATTR` route through, so a bare-`Record` arm beside the `Wrapped` one serves
  every spelling at once.
- *What a read off an anonymous record carries — open.* The `NEWTYPE` arm hands
  back a field cell lifted out of the wrapper's envelope. Decide whether the bare
  form reuses that lift unchanged, or whether the absent nominal layer changes
  which region the projected cell is pinned into.
- *Receiver naming in the diagnostic — open.* The `TypeMismatch` arm carries a
  builtin slot name because that is what the argument view knows. Decide whether
  to thread the receiver's source spelling to the error site, or to switch this
  arm to a `ShapeError` that names the field and the operand's rendered type
  without claiming an argument.
- *Whether the tutorial gains an example — deferred.* Follows the shipped
  behavior; the doc pass at the end of the item decides.

## Dependencies

**Requires:** none — the arm is local to the field-read chokepoint.

**Unblocks:** none tracked yet.
