# Type cells in value slots

**Problem.** A first-class type stored in a container rides the aggregate cell's
[`Held::Type`](../../src/machine/model/values/carried.rs) arm, and
[`KType::matches_held`](../../src/machine/model/types/ktype_predicates.rs) classifies it
against a slot by **exact handle identity** — `_ => self == *t`, line 501. So a concrete
proper-type slot admits the type value whose identity equals it: the type `Number` fills a
`:Number` element slot, and a signature type value fills its own `:S` slot. Its sibling
classifier `matches_type` (line 616) answers the same question for the same cell and rules
both `false`, comparing against the `OfKind(ProperType)` marker a non-signature type carrier
reports (line 629) rather than against the slot. The two doors disagree, and
`matches_value`'s `List` / `Dict` / `Record` arms all route through `matches_held`, so the
disagreement is reachable wherever an annotated boundary checks contents.

The disagreement launders values. A list holding a type value memoizes `List<ProperType>`,
which dispatch correctly refuses at a `:(LIST OF Number)` slot — dispatch trusts the carried
element type and never walks contents. The FN return boundary *does* walk contents, through
`matches_value`, so:

```koan
FN (SNEAK) -> :(LIST OF Number) = ([Number])
LET xs = (SNEAK)
FN (TAKE ns :(LIST OF Number)) -> Str = ("got numbers")
PRINT (TAKE xs)
```

passes the boundary, **stamps** the carrier `List<Number>`, and prints `got numbers`. After
the stamp the memoized type is a lie about the contents, and every later dispatch trusts it,
so one crossing of the weaker door admits the value everywhere the stronger one guards.

**Acceptance criteria.**

- `matches_held`'s `Held::Type` arm and `matches_type` return the same verdict for every
  (slot, type value) pair — the two classifiers answer the channel question once.
- A concrete proper-type slot admits no type cell: a list holding the type `Number` does not
  satisfy `:(LIST OF Number)` at any boundary, and a signature slot admits no signature type
  value.
- The FN return boundary rejects `FN (SNEAK) -> :(LIST OF Number) = ([Number])` instead of
  stamping it, and a test observes the refusal.
- A type-accepting slot still admits type cells: an `OfKind` element slot takes a stored type
  whose `kind_of` it subsumes, and `Any` takes any cell.
- No container's memoized carried type can disagree with what its own cells classify as at an
  annotated boundary — dispatch and ascription rule alike.

**Directions.**

- *Fix shape — open.* Either the `Held::Type` arm delegates to `matches_type`, or
  `matches_held` keeps a rule of its own and `matches_type` is restated against it.
  Recommended: delegate. `matches_type` already encodes the channel wall — a signature slot
  refuses a type value because a module is what satisfies a signature, and a concrete slot
  compares against the carrier marker rather than the type's own identity — and the arm
  encodes none of it. Delegation also deletes the second spelling rather than reconciling two.
- *Whether type cells belong in aggregates at all — decided.* They do: `Held::Type` exists so
  a list may hold a first-class type, and `List<ProperType>` is the correct carried type for
  one. Only the slot classification is wrong; the representation is not.
- *`Any` and `OfKind` arms — decided.* Unchanged. Both classifiers already agree there, and
  the acceptance criteria hold them fixed so the fix cannot narrow a type-accepting slot.
- *Union arms — decided.* Follow `matches_type`'s member delegation, which flips two
  `matches_held` verdicts (both forced by the first acceptance criterion): a union with a
  type-accepting member (e.g. `Number | ProperType`) admits a type cell a member admits,
  and a union type value stored in a cell of its own union slot is refused instead of
  sneaking through exact identity. A union of concrete members refuses a type cell as
  before.

## Dependencies

**Requires:** none — the fix is local to the two predicates.

**Unblocks:** none tracked yet.
