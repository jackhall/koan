# Tightness-audit coverage

Two blind spots in the shipped reach-tightness report, both of which make a real
over-fold read as clean.

**Problem.** The tightness audit
([memory-model.md § Debug region audits](../../design/memory-model.md#debug-region-audits))
compares the regions a fold pins against the addresses its product actually
embeds, and flags the difference. It sees two things it should not miss.

*Uninstrumented fold sinks.* Only `StepAllocator::alloc_carried_with`
([step_allocator.rs](../../src/machine/core/arena/step_allocator.rs)) carries the
instrumentation. The relocation verbs `Delivered::transfer_into` and
`Delivered::merge_into` ([delivered.rs](../../workgraph/src/witnessed/delivered.rs))
compose operand reach into a product the same way and are the accumulating fold
every multi-operand koan value is built through, so an over-fold that happens
there is invisible. The audit's walker is Koan-typed — it matches on `KObject`
arms — while the two verbs are library-generic, so there is no instrumentation
point shared with the chokepoint already covered.

*Unwalked captured bindings.* `collect_addresses`
([reach_audit.rs](../../audit/reach_audit.rs)) records a `KFunction`'s
and a `Module`'s own address and captured-scope pointer, but does not descend
into that scope's binding table ([scope.rs](../../src/machine/core/scope.rs)). A
product whose only route to an operand runs through a captured binding therefore
intersects nothing, reads as non-contributing, and draws a flag the fold does not
deserve — a false positive, the failure mode that makes an audit ignorable.

**Acceptance criteria.**

- A fold through `Delivered::transfer_into` runs the same contribution
  comparison the chokepoint runs, and a test whose transfer pins an operand its
  product never embeds observes a flag naming that operand's home.
- A fold through `Delivered::merge_into` does likewise, with its own test.
- The address walk reaches values held in a captured scope's binding table, and
  a test whose product embeds an operand only through a captured binding
  observes no flag.
- Every addition stays under the existing `region-audit` gate, so a release
  build carries none of it.

**Directions.**

- *Where the relocation-verb instrumentation lives — open.* (a) At the Koan call
  sites that drive the two verbs, each opening its own `FoldAudit`; (b) inside
  the library verbs behind a debug gate, with the embedder supplying the address
  walk through a callback. Recommended: (a) — the walker is Koan-typed, and (b)
  would put an embedder callback on the two hottest relocation paths in the
  library for a diagnostic's sake.
- *Whether an unwalked-binding false positive is instead cut by narrowing the
  claim — open.* (a) Descend the binding table, as the criteria state; (b) treat
  a product embedding a captured-scope pointer as contributing on its owning
  operand outright, skipping the descent. (b) is cheaper and cannot false-flag,
  but coarsens every closure-bearing fold to "contributed" and so hides genuine
  over-folds behind a captured scope.
- *Cycle-detector coverage — deferred.* The pin-ring detector's own gaps (it
  reports one ring per retained member, and reports rather than prevents) are a
  separate question from this audit's coverage and are recorded in the
  [hotspot map](README.md#hotspot-map) rather than here.

## Dependencies

Both audits shipped together; this item widens the tightness half of them and
leaves the pin-ring detector alone.

**Requires:** none — additive diagnostics over a shipped audit.

**Unblocks:** none tracked.
