# Definitions park on free names

A function definition waits for every in-flight binder its body names, so a callable commits only
once its whole environment is bound.

**Problem.** A `FN` / `EXPR` / `OP` definition commits the moment its own statement finishes,
whatever its body names. A free name in the body that is still an in-flight placeholder in the
defining chain is not consulted at definition: the callable is born over a scope holding a claim,
and the park happens later, at whichever call first reads the name
([name-placeholders.md](../../design/execution/name-placeholders.md)). Two consequences sit in the
code today:

- The environment copy's readiness gate refuses any chain with a standing claim
  ([`Scope::is_copy_ready`](../../src/machine/core/scope.rs), `has_no_claims`), so a closure that
  escapes while a sibling binder is still finalizing pins its producer frame instead of
  consolidating — a definition can be "done" while its environment is not.
- Recursion rides on claims rather than on declarations: a self-referencing `LET f = FN … f …` and
  two sibling `EXPR`s that call each other are legal only because the body's read parks at call
  time on the sibling's claim. The claim store is therefore load-bearing for a language whose data
  and bindings are immutable, where a definition's environment could in principle be settled before
  the definition commits.

**Acceptance criteria.**

- A definition whose body names a still-in-flight binder in its defining chain parks on that
  binder's producer — the same claim edge a value read parks on — and commits only after every
  such binder has committed; the free-name set it parks on is the one `CLOSE` inference already
  enumerates ([close_inference.rs](../../src/machine/model/close_inference.rs)).
- At the moment a callable commits, every free name of its body resolves to a committed binding
  or to nothing at all; no claim is standing in its captured chain for a name the body reads.
- Self-recursion and mutual recursion between co-declared definitions are expressed through a
  declaration window — the mutually-visible, order-independent announcement a module body already
  gives its nominal types — rather than through call-time parks on claims, and the existing
  recursive programs in the suite still run.
- The copy-readiness gate's "no standing claim" half never declines a chain a committed callable
  captured; a closure escaping its frame consolidates or pins on cost alone.
- The full slate passes and the Miri slate is clean.

**Directions.**

- *Which free-name reader — decided.* The `CLOSE` inference walk, which already answers "what
  would this body resolve outward for" against the defining chain with the positional cutoff
  applied; a second enumeration is not admitted.
- *How co-declared definitions see each other — decided.* Through a declaration window shaped as
  [one-declaration-window.md](../refactor/one-declaration-window.md) settles it for types: a block's
  definitions announce their names up front and elaborate against the window, so a body naming a
  sibling reads an announced identity rather than parking on a claim. Extending that window from
  type declarations to value definitions is this item's work.
- *Where the wait lives — open.* Either the definition's finalize parks on the outstanding
  producers (the `AwaitDeps` shape `CLOSE OVER` uses for its implicit close), or the fan-out
  installs the dependency edges at submission from the plan alone. Recommended: the finalize park —
  it reuses the existing claim edge and needs no new submission-time machinery.
- *Value-channel claims after this ships — open.* If no body can read a claimed name at call time,
  the value channel's claims are consulted only by definitions and by top-level statements; whether
  the claim store's value channel shrinks to a definition-time structure is decided once the
  parking is in and measured.

## Dependencies

The slotted per-call scope fixes what a frame holds up front; the one-window representation is the
shape the announcement reuses. Both are prerequisites, not soft ordering.

**Requires:**

- [Slot-shaped per-call scopes](../reduce_allocs/slot-shaped-per-call-scopes.md) — a frame's own
  binding set is static and owned by the value store before this item touches what a definition
  waits on.
- [One declaration-window representation](../refactor/one-declaration-window.md) — the single
  window type co-declared definitions announce through.

**Unblocks:** none tracked yet.
