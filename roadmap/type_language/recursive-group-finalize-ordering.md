# RECURSIVE TYPES group finalize ordering

**Problem.** A `RECURSIVE TYPES` group whose member schema references a
*sibling binder's* variant through the qualified sigil panics instead of
sealing or erroring cleanly. Given

```
RECURSIVE TYPES Group = (
  UNION Tree = (Leaf :Number Node :(Other Alt))
  UNION Other = (Alt :Number Branch :Tree)
)
```

the run aborts with
`projected_schema on an unfilled member — finalize must run first`
([recursive_set.rs](../../src/machine/model/types/recursive_set.rs), the
`projected_schema` expect). One member's schema projects — reading a
cross-referenced member's schema off the shared `RecursiveSet` — before that
member's own finalize has filled its `pending` slot, so the group finalize
ordering lets `projected_schema` run against an unfilled member. The panic is
user-reachable from ordinary source, is a hard abort rather than a typed
error, and is independent of arm dispatch: it reproduces on a cross-binder
sigil between two group members and does not need the union-under-seal
sibling-variant path
([user-types.md § Unions dissolve into per-variant newtypes](../../design/typing/user-types.md#unions-dissolve-into-per-variant-newtypes)),
which resolves a member's reference to a variant of *its own* union.

**Acceptance criteria.**

- A `RECURSIVE TYPES` group whose member schema references a sibling binder's
  variant via the qualified sigil either seals to the correct `SetLocal`
  cross-reference and resolves at projection, or surfaces a typed `KError` —
  never a panic.
- Reading a member's schema off the shared set observes it only after that
  member's finalize has filled its slot, so `projected_schema` never runs
  against a `pending` member during group sealing.
- The full test suite and the Miri audit slate are green across the change.

**Directions.**

- *Fix shape — open.* The root is group finalize ordering — a member's schema
  is projected before the cross-referenced binder's finalize fills it. Options
  span (a) ordering the group's member finalizes so every cross-referenced
  slot is filled before any dependent member projects; (b) deferring the
  cross-reference projection until all group members have sealed; (c)
  detecting the unfilled-member read and surfacing a typed error where a clean
  seal is not yet reachable. Not yet scoped — this item names the fault, not
  the fix.

## Dependencies

Surfaced while shipping MATCH-as-ordinary-type-dispatch (the sibling-variant
sigil under seal); the two share the qualified-sigil surface but the fault is
independent — it lives in group finalize ordering, not arm dispatch. The
[interned-type-content](../type_memos/interned-type-content.md) flip rewrites group
sealing wholesale (per-SCC seal over a scope-carried window) and carries this
reproduction as a phase gate, so this item retires with the flip unless the panic
warrants a standalone fix sooner.

**Requires:** none — the fault is in existing `RECURSIVE TYPES` group sealing.

**Unblocks:** none.
