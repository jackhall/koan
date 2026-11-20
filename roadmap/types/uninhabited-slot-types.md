# A slot type no value inhabits

A record type with a `Never` field, read as the `Never` it is in the class
walk.

**Problem.** *Fits*' shape clause and the per-class ranking verdict walk a
shape's [priority classes](../../lattice/src/types/solving.md#priority-classes)
with the other shape's slot types as arguments, each standing for every type a
call carries under it. A slot type exactly `Never` stands for no call, and the
walk leaves its slot out (`ClassWalk::over_slot_types` in
[`ranking.rs`](../../lattice/src/types/ranking.rs)). A slot type no value
inhabits that is not the node `Never` — `{x :Never}`, which
`{x :Number} & {x :Str}` meets to — is read as a call: the class first
mentioning a variable fixes it at that type, and a later class refuses the
other side's rigid variable against it. So *fits* is not transitive through a
written-order shape with such a slot. A probe
(`probe_an_uninhabited_record_slot_in_a_solving_class`, kept out of the tree)
shows the break.

**Acceptance criteria.**

- *Fits* is transitive through a written-order shape whose slot type is a
  record, list or dict type no value inhabits, and a law over generated
  chains draws such a slot.
- `{x :Number} & {x :Str}` at a slot solves no variable, as `Never` there
  solves none.

**Directions.**

- *How the walk sees an uninhabited type — open.* (1) Canonicalize a
  composite with an uninhabited part to `Never` where it is built, so the
  walk's exact-`Never` test covers it. (2) Test "uninhabited" in the class
  walk. Recommended: 1, which keeps one spelling of the empty type.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
