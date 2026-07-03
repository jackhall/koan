# The resolve-or-await protocol combinator

**Problem.** The resolve-a-type-or-await-its-producer protocol is hand-rolled
per site. [newtype_def.rs](../../src/builtins/newtype_def.rs):131-171 matches
`resolve_type_with_chain` — `Bound` finalizes, `Parked(producer)` emits
`Action::AwaitDeps` on `DepRequest::Existing` with a finish that re-runs the
same match, `None` errors — with the resolve arms duplicated inside and
outside the wake closure.
[fn_def/return_type.rs](../../src/builtins/fn_def/return_type.rs) rolls the
same protocol against `resolve_type_identifier` (`Done` / `Park` / `Unbound`),
carrying parked producers to the FN-def's aggregated dep list and re-resolving
in `resolve_capture_at_finish`, with its own second-park protocol error
("FN return type parked after dep-finish wake").
[val_decl.rs](../../src/builtins/val_decl.rs) carries the adjacent shape (a
leaf type re-resolved against `decl_scope` at dep-finish). Each site re-states
the protocol's invariants — re-resolve on wake, a second park is a protocol
error, unbound-after-wake is a hard miss — and their diagnostics have already
drifted apart.

**Acceptance criteria.**

- A named Koan-side combinator above `Action` owns resolve-or-await: a caller
  states the identifier, the scope/chain, and the on-resolved continuation;
  park-on-producer, re-resolve-on-wake, and the second-park protocol error
  appear once, in the combinator.
- `newtype_def.rs`, `fn_def/return_type.rs`, and `val_decl.rs` route the
  combinator; none contains a hand-written parked → await → re-resolve match,
  and the second-park error has one shape (each site keeps its slot-name
  diagnostic prefix, e.g. "NEWTYPE repr slot" / "FN return-type slot").
- Existing tests green; no diagnostic regressions beyond unified wording.

**Directions.**

- *Combinator currency — open.* (a) An envelope combinator returning
  `Action::AwaitDeps` directly — serves newtype-style immediate parks only;
  (b) a resolve-classification half plus a re-resolve-at-wake half, composed
  by each site — serves both the immediate park and `fn_def`'s aggregated dep
  list. Recommended: (b); `return_type.rs`'s producers ride the FN-def's
  single `AwaitDeps`, which (a) cannot express.

## Dependencies

**Requires:** none — a Koan-side consolidation above `Action`, independent of
the library boundary.

**Unblocks:**

- [The step construction context](step-construction-context.md) — its
  finish-signature migration touches every `AwaitContinue` site; collapsing
  the hand-rolled ones first keeps that migration to combinator internals.
