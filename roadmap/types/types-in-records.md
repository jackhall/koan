# Types held in record values

A record value holds a type under a type name, and a value name never holds
one.

**Problem.** A record field's name does not decide whether it may hold a type,
and a type a record value holds cannot be read back:

- `LET r = {t = :Number}` loads: a value-named field holds a type.
- `LET r = {Ty = 5}` loads: a type-named field holds a number.
- `LET r = {Ty = :Number}` loads, typed `:{Ty :ProperType}`, but `r.Ty`
  cannot read the type back: read as a value or in a type expression,
  `:(r.Ty)`, it loads and faults at run with "`:{Ty :ProperType}` has no
  member Ty". On a parameter `r :{Ty :Type}`, the same read faults at the
  call.
- `NEWTYPE Holder = :{Ty :Type}` refuses with "`Ty` names no binding visible
  here", though `:{Ty :Type}` is accepted as a parameter's type.

A module's type member, by contrast, is read through a path, `m.Carrier`
([comparing two paths](../../design/modules.md#comparing-two-paths)).

**Acceptance criteria.**

- `LET r = {t = :Number}` refuses the load: a value-named field holds no type.
- `LET r = {Ty = :Number}` loads, and `r.Ty` reads the type it holds.

**Directions.**

- *Whether `:(r.Ty)` is a type, as `:(m.Carrier)` is — open.* A path through a
  record value would read its type off the record value the root holds at
  run, so a static type naming it keeps it only where the reader's frame
  reaches such a record value, as for a module's member. A record value built
  in place over load content is closed; one over a parameter is open, which
  needs record values to carry a content tree and identity, as modules do.
- *Whether a type-named field must hold a type — open.* `{Ty = 5}` refused, or
  left as a value under a type name.
- *Whether the rule reaches `LET` — open.* `LET t = :Number` loads today, as
  `{t = :Number}` does.
- *A type-named field in a `NEWTYPE` — open.* Why `:{Ty :Type}` refuses there
  and not as a parameter's type.

## Dependencies

Where `:(r.Ty)` becomes a type, a path through a record value is reached by
content as a module's is ([paths](../../src/dispatch/README.md#paths)).

**Requires:** none.

**Unblocks:** none.
