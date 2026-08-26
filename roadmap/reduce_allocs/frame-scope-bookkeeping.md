# Frame and scope bookkeeping

**Problem.** The dhat profile of the audit shapes attributes ≈28 allocations per
tail-loop step to per-hop frame and scope construction. A fresh tail hop mints a whole
frame stack: `CallFrame::new`
([src/machine/core/arena/frame.rs](../../src/machine/core/arena/frame.rs), 4/step) builds
a fresh `Rc<FrameStorage>` and the `Rc<CallFrame>` shell — one of those mints is the arm
frame `arm_tail` ([src/builtins/branch_walk.rs](../../src/builtins/branch_walk.rs)) makes
unconditionally so the selected body has somewhere to bind `it`, whether or not the arm
reads it — and
`build_frame_child_witnessed` (6/step) mints the hop's region and bumps the child scope
and its delivery envelope into it. The slot layer allocates alongside: every tail replace
mints a fresh `Rc<SlotFrame>`
([src/machine/execute/nodes.rs](../../src/machine/execute/nodes.rs)), and
`LexicalFrame::push`
([src/machine/core/lexical_frame.rs](../../src/machine/core/lexical_frame.rs)) mints an
`Rc` per pushed chain link. The largest single site is the binding seam:
`Scope::adopt_carried`
([src/machine/core/scope/reach.rs](../../src/machine/core/scope/reach.rs), 9/step)
re-homes each delivered value into the consuming scope. All of it is
build-use-drop within one hop — the loop's steady state churns through allocations whose
lifetime is exactly one iteration.

**Acceptance criteria.**

- A steady-state tail hop mints no fresh frame bookkeeping: a re-profile of
  `audit/shapes/tail_loop_steps100.koan` attributes no per-step term to `CallFrame::new`,
  `build_frame_child_witnessed`, `SlotFrame` minting, or `LexicalFrame::push`.
- `Scope::adopt_carried`'s nine per-step allocations are attributed site-by-site, and
  every one not required by a genuine cross-hop escape is removed.
- The recorded tail-loop baseline in [audit/README.md](../../audit/README.md) drops by
  this item's share, and `tests/allocation_baseline.rs` is re-measured to the new figure.

**Directions.**

- *Frame recycling — open.* Whether a `FreshTail` hop can reuse the retiring incarnation's
  `FrameStorage` and region memory instead of minting fresh ones. The soundness question
  is escapees: an escaping closure pins the retiring storage
  (`CallFrame::storage_rc`), so reuse is only sound when nothing extended the pin — the
  hop would need to observe that and fall back to a fresh mint when it can't. Alternatives:
  detect-and-reuse at the replace site, or keep per-hop storage and cut only the layers
  above it.
- *Slot and chain minting — open.* `SlotFrame::replacing`/`opening` and
  `LexicalFrame::push` each mint an `Rc` per hop for state that is
  build-use-drop within the hop. Whether these follow the frame (recycled with it),
  become region-placed alongside the scope they describe, or stay heap-minted as an
  accepted remainder is undecided.
- *The adoption seam — open.* What `adopt_carried`'s nine allocations are — retention
  bookkeeping, envelope re-anchoring, or relocation copies — is not yet attributed below
  the function level; the `Pin` disposition adopts in place, so the question is which
  seam forces `Relocate` (or allocates on the `Pin` arm) in the steady state.
  Attribution first, then the cut.

## Dependencies

**Requires:** none.

**Unblocks:** none.
