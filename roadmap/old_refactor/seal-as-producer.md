# Seal as a value-bearing producer

A forward reference into an open declaration window takes one dep on the group's seal and reads
the sealed identities from that dep's terminal, like any other value dependency.

**Problem.** `FieldListDeferral`'s `awaited_producers`
([field_list.rs](../../src/machine/execute/decide/field_list.rs)) are the one place in the language
where a dep is notify-only: the finish never reads those terminals. It re-walks the field list and
resolves the forward names from the now-populated scope, so `into_parts` slices its own dep vector
into `[awaited_producers ++ sub_dispatches]` and hands the re-walk only the tail from `first_sub`.
Everywhere else a consumer takes its value straight from its producer's terminal. The same set is
threaded through [fn_def/finalize.rs](../../src/builtins/fn_def/finalize.rs),
[fn_def/signature.rs](../../src/builtins/fn_def/signature.rs) and
[nominal_schema.rs](../../src/builtins/nominal_schema.rs).

The notify-only shape is forced by the seal protocol:

- There is no 1:1 dep↔slot mapping. `park_until_seal`
  ([resolver.rs](../../src/machine/model/types/resolver.rs)) parks a forward reference on the
  producers of **every** unfilled member, deduplicated across names. One name awaits N producers and
  several names share one producer set, so there is nothing to feed positionally.
- The producer terminal is often not the named type. A co-declared member whose fill does not close
  the group terminalizes as `Null` (`seal_outcome_into_carrier`,
  [newtype_def.rs](../../src/builtins/newtype_def.rs)); the identities are installed all at once by
  the seal's binding writes (`seal_writes`). A binder name resolves to the union node, and a member's
  absolute handle is computed by the seal over the whole reference structure. Those answers exist
  only in the post-seal environment, hence the re-walk.

Standalone in-flight declarations already have a value-bearing terminal: `NameLookup::Parked(edge)`
parks on one edge whose terminal is the sealed type carrier.

**Acceptance criteria.**

- The seal of a declaration window is a scheduler producer whose terminal carries the sealed
  window's name → `KType` map: every standalone member's absolute handle plus each binder's union.
- A consumer parking on an open window takes exactly one dep, on that window's seal producer, and
  `park_until_seal` no longer collects the unfilled members' producer set.
- Every dep a field-list finish waits on is consumed by value: `FieldListDeferral` carries no
  notify-only producer set, and `into_parts` returns no `first_sub` split.
- `awaited_producers` is gone from `fn_def/finalize.rs`, `fn_def/signature.rs` and
  `nominal_schema.rs`, replaced by the seal dep those paths thread today's set for.
- Every identity, forward-reference and mutual-recursion behavior the current suite pins is unchanged.

**Directions.**

- *How the re-walk resolves forward names — open.* The re-walk itself stays: the sub-Dispatch DFS
  feed needs it regardless (the first walk yields `KType::ANY` placeholders and no slot indices).
  Candidates: the re-walk resolves forward names against the seal terminal's map (pure value flow),
  or the seal producer's terminal is ordering only and the map is still installed via `WriteOp::Type`
  so the re-walk's scope read is an ordinary read (smaller change). Recommended: the map on the
  terminal — the point of the item is that the dep carries what it means.
- *Who owns the seal producer node — open.* Today the seal is a side effect of whichever member
  statement fills last. A seal producer's identity must exist at announce time, when the window's
  members are pre-scanned, before it is known which statement closes the group. Candidates: the
  window allocates the node when it opens and the closing fill terminalizes it, or the announcing
  step schedules it as a dep-finish over every member's placeholder producer.
- *Carrier for the sealed window — open.* Windows have no carrier representation. Every `KType` is
  an interned `Copy` handle, so the name → handle map is region-free and a shallow carrier suffices;
  whether it is a new carrier kind or a `Record` of type values is undecided.
- *Standalone declarations — open.* Unify `NameLookup::Parked(edge)` under the seal producer so a
  standalone declaration is a one-member window, or leave that path taking the type carrier's
  terminal directly. Recommended: unify, so there is one park shape.

## Dependencies

**Requires:**

- [One declaration-window representation](one-declaration-window.md) — the seal terminal carries the
  window; there should be one window type to carry.

**Unblocks:** none tracked yet.
