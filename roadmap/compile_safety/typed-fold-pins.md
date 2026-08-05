# Typed pins for the pinned fold verbs

**Problem.** The externally-pinned verbs on `Witnessed` — `with_pinned` /
`map_pinned` / `map_pinned_placing` / `merge_pinned` / `merge_pinned_placing`
([witnessed.rs](../../workgraph/src/witnessed.rs)) — accept `pin: &Pin` with
only `Pin: Witness`: no type links the pin to the operand backing it must keep
alive across the transient re-anchor, so a call passing a witness unrelated to
the operand re-anchors over unpinned backing — dangle-capable safe code.

Every production pin is engine-derived today, so the hazard is a surface, not
a live call site: `StepContext::alloc_with` and `StepContext::map_pinned_placing`
([step_ctx.rs](../../workgraph/src/witnessed/step_ctx.rs)) pin with the
context's own held frame; `Delivered::project`
([delivered.rs](../../workgraph/src/witnessed/delivered.rs)) pins with the
envelope's own bundle; and the relocation/merge verbs (`transfer_into` /
`transfer_into_placing` / `merge_into` / `merge_into_placing`) run the
crate-private `merge_composed` engine under pins derived from the envelopes
themselves, with destination coverage stripped/re-unioned by the engine, not
asserted by the caller. `merge_pinned` / `merge_pinned_placing` have **no
production callers at all** — only workgraph and koan tests exercise them (and
tests also call `map_pinned` / `with_pinned` with free-chosen pins, which is
exactly the shape the surface should reject).

The obligation the untyped surface leaves is discharged in doc prose on the
verbs ("`pin` is held for the whole call and keeps the pointee live";
`merge_pinned`'s destination operand "covered by its own live destination,
which the caller necessarily holds") rather than by type. The envelope path is
the typed precedent: `Delivered`'s verbs bundle value and pins, and the caller
supplies nothing.

**Acceptance criteria.**

- No public verb accepts a pin unrelated to the operand it covers: the pin is
  tied to the backing by type (a bound or a bundled value), or the verb is
  confined so only engine code that derives the pin can call it. A call
  passing an unrelated witness is a compile error.
- `merge_pinned` / `merge_pinned_placing` no longer exist; the
  `ComposeWitness` semantics they exercised are tested through the
  crate-private `merge_composed` engine.
- No verb doc carries a caller-facing "you must hold a covering pin"
  obligation — the pin-liveness prose is the typed story only.

**Directions.**

- Link mechanism — decided per the shipped production paths: bundle the pin
  with the operand as `Delivered` does (carriers travel with their hosts;
  the verbs take no pin parameter). Every live call site already goes
  through it or through `StepContext`'s own-frame pinning; the residual
  work is closing the loose surface by demoting the pin-taking verbs to
  crate-/module-private behind the engine wrappers.
- Scope — decided: the `Witnessed` verb family only. The `Sealed::open_with`
  / `open_at` siblings stay as they are, with no follow-up item.
- Fate of the production-dead `merge_pinned` pair — decided: delete both;
  their two workgraph tests (which test `PinBundle` composition, not the
  pin channel) rewrite over the crate-private `merge_composed` engine.
- Koan-side tests that pass free pins (`arena/tests.rs`, `lift/tests.rs`) —
  decided: envelope-shaped rewrite (read through `Delivered`; a by-ref
  `Delivered::open_ref` covers the non-`Copy` reads), no cross-crate test
  door. `StepCarried::inspect_pinned` retypes to take the anchor's owner.

Plan: `scratch/typed-fold-pins-plan.md`.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none.
