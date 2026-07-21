# One deferral bundle for field-list dispatch

**Problem.** The three field-list deferral entry points in
[field_list.rs](../../src/machine/execute/dispatch/field_list.rs) —
`defer_field_list`, `defer_field_list_action`, and
`defer_field_list_action_composed` — each take ~10 arguments under a
`#[allow(clippy::too_many_arguments)]`, each rebuild the same
`FieldListRewalk` from the same seven parameters, and each assemble the same
`[park ++ subs]` dependency vector (two via `field_list_deps`, one inline).
The Action/Outcome currency split between them is real; the triplicated
state-assembly is pure duplication, and a parameter added to the rewalk means
extending three signatures and three call paths by hand
([field_list.rs](../../src/machine/execute/dispatch/field_list.rs),
[nominal_schema.rs](../../src/builtins/nominal_schema.rs),
[parameterized_types.rs](../../src/builtins/parameterized_types.rs)).

**Acceptance criteria.**

- A single bundle type owns the field-list deferral state (expression,
  park producers, sub-dispatches, context, name kind, threaded value, window,
  chain, pending guard, error frame); the three entry points are finishing
  methods on it, and the `[park ++ subs]` dependency vector is assembled in
  exactly one place.
- `field_list.rs` carries no `#[allow(clippy::too_many_arguments)]`.
- Dispatch behavior is unchanged — the same parks and subs install with the
  same placeholders — with existing tests green.

**Directions.**

- *Bundle shape — open.* (a) a `FieldListDeferral<'a>` struct with three
  consuming finish methods (`.outcome(compose)`, `.action(finalize)`,
  `.action_composed(compose)`); (b) collapse the Action/Outcome currencies
  first and let one finish method serve all three callers. Recommended: (a) —
  the currency split reflects real caller differences (builtin bodies return
  `Action`, the dispatch walk wants `Outcome`), and (b) grows the seam this
  item is shrinking.

## Dependencies

**Requires:** none — self-contained dispatch cleanup.

**Unblocks:** none tracked yet.
