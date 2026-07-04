# Total carrier resolution for spliced names

Guarantee 2 of [design/scheduler-library.md](../../design/scheduler-library.md)
— a reach set always names every region a value's borrows can reach, and no
caller can assert one — holds at the name-splice surface.

**Problem.** `part_walk`'s wrap-slot arm
([keyworded.rs](../../src/machine/execute/dispatch/keyworded.rs) ~344) seals
a resolved bound name's splice cell from the scope carrier lookups
(`resolve_value_carrier` / `resolve_type_carrier`), but when a
`NameOutcome::Resolved(c)` co-occurs with a carrier-lookup miss it falls back
to `Sealed::seal(Witnessed::resident(*c))` — wrapping a pre-existing
reference in an asserted-empty-reach witness, outside `Region`. The cell's
soundness rests on the convention that a resolved-but-unbound name is always
a region-pure builtin, not on a type: if any resolution source yields a
`Resolved` value with real foreign reach whose name is not in binding storage
(the type channel is the least-exercised path), the cell under-witnesses, a
later `adopt_sealed` folds nothing, and the re-anchored borrow dangles once
the producer frame drops. The witnessed substrate's construction discipline
is that `resident` stays confined within `Region` and never wraps an existing
reference.

**Acceptance criteria.**

- Every `NameOutcome::Resolved` splice in `part_walk` seals a carrier
  obtained from the resolution surface; the `Witnessed::resident` fallback
  arm is deleted, so a resolved name without a carrier no longer
  type-checks rather than sealing empty reach.
- The scope carrier lookups are total over resolvable names: the
  region-pure/builtin path returns an explicitly-constructed region-pure
  carrier instead of a miss.
- No dispatch-internal production code wraps an existing reference in
  `Witnessed::resident`; the construction appears only inside the region
  engine and (if kept) test fixtures.
- A test covers the type channel: a first-class type resolved as an inline
  splice arrives with its binding's stored reach, and adopting the cell pins
  the type's producer region.

**Directions.**

- Where the region-pure carrier is minted — open. (a) Extend
  `resolve_value_carrier` / `resolve_type_carrier` to return a carrier for
  every resolvable name, minting the region-pure case internally (the lookup
  becomes total); (b) keep the lookups partial and mint at the `part_walk`
  site through a named `Scope` verb. Recommended: (a) — totality at the
  lookup is what makes the fallback deletable.
- Fallback deletion — decided. After totality, no `_ =>` arm remains at the
  splice site; the non-carrier case is unrepresentable.
- `test_support::spliced_part`'s test-only `Witnessed::resident` construction
  — open. Either it stays as a fixture-minting convenience (tests build
  region-pure values by construction) or it routes the same total surface.

## Dependencies

**Requires:** none — a self-contained totalization of the existing lookup
surface.

**Unblocks:** none — closes a guarantee leak; nothing is scoped against it.
