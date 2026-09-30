# A value's type is its surface

Every read of a container sees what its carried type names, at the type it
names it, at every depth.

**Problem.** A retype restamps a container's top node alone
(`Value::retyped` in [`values`](../../src/values.rs)), and every reader reads
the cells beneath it: `ATTR`'s and `FROM`'s `field` in
[`builtins`](../../src/dispatch/builtins.rs), the
[renderer](../../src/values/render.rs), [equality](../../src/values/equality.rs)
and the [deep copy](../../src/values/crossing.rs), each on its own.

- After `LET r = ({x = 1, y = "a"} :! :{x :Number})`, `r` carries
  `{x :Number}`. The load refuses `r.y`, yet a read it cannot type,
  `ATTR (HIDE r) y` through `EXPR #(HIDE v :Any) -> Any = #(v)`, reads `"a"`;
  `PRINT r` prints `{x = 1, y = a}`, and `r == {x = 1}` is false.
- `ATTR` hands a field back at the record's type there only because its
  [type rule](../../src/dispatch/README.md#the-builtin-table) retypes the value
  after the native reads it; no other reader retypes a part.
- `FROM` (`Native::Project`) builds a new record from its fields' values rather
  than restamping its record.
- A written list naming a field twice, `#[x y x] FROM r`, hands `Record::new` a
  repeated name and trips its debug assertion that a record's field names are
  distinct.

**Acceptance criteria.**

- Every read of a record, list or dict goes through one door in
  [`values`](../../src/values/README.md). It sees only the fields the carried
  type names, and it hands back each field, element or entry retyped to its type
  there. After `LET r = ({x = 1, y = "a"} :! :{x :Number})`,
  `ATTR (HIDE r) y` faults with no field `y`, `PRINT r` prints `{x = 1}`, and
  `r == {x = 1}` is true.
- A retype restamps a value and copies nothing. A copy across regions lays down
  only what the value's type names.
- `#[x] FROM r` is `r` retyped to the projection of its carried type onto `x`,
  over `r`'s own cells.
- A list naming a field twice projects it once, and a name the record's carried
  type does not name faults, as `r.y` does above.

**Directions.**

- *Hide on retype, drop on copy — decided.* A retype and `FROM` restamp the
  value and share its cells, and the door hides what the type does not name. A
  copy, which lays the value down anyway, drops it, as
  [slicing and splicing](../metaprogramming/slicing-and-splicing.md)'s views
  resolve at a crossing.
- *A retype applies on read — decided.* A read retypes the part it hands back,
  so a retype never walks the value, and a nested part obeys it all the same.
- *A shallow schema — decided.* `Schema FROM record` takes a list of names and
  projects the record's top level alone.
- *A nested schema — deferred.* It is [a nested projection](nested-projection.md)'s.
- *A repeated name — decided.* The call projects each field once, as the load's
  projection already does.
- *What a copy's weight counts — open.* A value with hidden parts weighs every
  cell it holds, while its copy lays down fewer. A
  [weight](../../src/values/README.md#weight) already counts a knot once per
  reference, so the over-count may stand, or a restamp may reweigh.

## Dependencies

**Requires:** none.

**Unblocks:**

- [A nested projection](nested-projection.md) — the shallow projection it nests.
