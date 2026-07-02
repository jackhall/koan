# One chain-walk for scope name resolution

**Problem.** `src/machine/core/scope.rs` implements the ancestor-walk lookup ladder
five times — `resolve_with_chain` (line 747), `resolve_value_carrier` (765),
`resolve_type_with_chain` (960), `resolve_type_reach` (986), and
`resolve_operator_group_with_chain` (1008) — each a
`self.ancestors().find_map(|s| s.bindings().lookup_X(name, s.binding_cutoff(chain)))`
with per-channel variation. The builtin-first root consult ("builtins are
authoritative — read the root in one hop, skip the walk") is verbatim in three of them
(969-971, 987-988, 1016-1018) and absent from the value ladders, so the rule's reach
is defined by which ladders happen to repeat it. `resolve_type_reach` re-implements
`resolve_type_with_chain`'s walk end to end just to return the binding's stored reach
instead of the type.

**Acceptance criteria.**

- The ancestors-with-cutoff walk is expressed once, parameterized by the per-scope
  probe; the five resolution entry points delegate to it.
- The builtin-first root consult is one helper composed into the type and operator
  ladders, not repeated inline per ladder.
- `resolve_type_reach` derives from the same walk as `resolve_type_with_chain` (no
  second hand-written type ladder), with memo-miss behavior unchanged.
- Resolution semantics unchanged — innermost-wins, placeholder shadowing, visibility
  cutoffs, `Parked` propagation — with existing tests green.

**Directions.**

- *Walk shape — open.* (a) A private generic
  `fn walk_chain<T>(&self, probe: impl Fn(&Scope) -> Option<T>) -> Option<T>` plus a
  small builtin-first combinator; (b) a lookup-spec enum driving one interpreter.
  Recommended: (a) — the ladders differ only in the per-scope probe and the optional
  root consult.
- *Reach derivation — open.* Fold `resolve_type_reach` onto the carrier lookup
  (`lookup_type_carrier` hit → stored reach; `Parked` / miss → empty set) vs keep a
  dedicated probe. Recommended: derive from the carrier hit — one probe, two
  projections.

## Dependencies

**Requires:** none — leaf cleanup.

**Unblocks:** none tracked.
