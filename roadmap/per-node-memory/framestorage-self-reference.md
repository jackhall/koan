# FrameStorage self-reference removal

The region↔child-scope self-reference is dissolved and its three audited `unsafe` tokens deleted;
what remains is confirming a clean full Miri slate once the pre-existing leak it reports clears.

**Problem.** The full Miri slate is red — it reports the pre-existing 1378-allocation process-exit
cycle leak that [alloc-witness-plumbing](alloc-witness-plumbing.md) owns — so a clean full-slate run
cannot yet confirm this restructure introduced no regression. The restructure itself has shipped: the
per-call child `Scope` rides the externally-witnessed [`SealedExtern`](runloop-cps-open.md) carrier
read through `SealedExtern::attach` (the frame's storage `Rc` as the witness), the seed binds reach
the region through the child scope's own `region` field, and all three audited `unsafe` tokens — the
`reattach_witnessed` `NonNull::as_ref`, the `with_frame_interior` `pin_deref` re-exposure, and the
`pin_deref` primitive — are gone (see
[memory-model.md § Region lifetime erasure](../../design/memory-model.md#region-lifetime-erasure)).
It adds only safe-signature accessors, and targeted Miri on the changed sites (the
`try_reset_for_tail` trio, the scope/`attach` round-trip, the `YokedChild` round-trip) is green —
0 leaks, 0 UB — so the residual slate red is the pre-existing cycle, not a regression here.

**Acceptance criteria.**

- Once [alloc-witness-plumbing](alloc-witness-plumbing.md) clears the pre-existing 1378-allocation
  leak, the full Miri slate is green — confirming this restructure adds no new leak or UB — and
  `cargo test` and `cargo clippy --all-targets` are clean.

**Directions.**

- *Confirmation, not new work — decided.* The restructure deletes `unsafe` and adds only
  safe-signature accessors, so the open item is a clean full-slate run after the leak fix, not a code
  change; targeted Miri already evidences no new leak/UB.

## Dependencies

The full-slate confirmation rides [alloc-witness-plumbing](alloc-witness-plumbing.md)'s leak fix
landing first; that is a sequencing note, not a prerequisite of the restructure (which has shipped),
so it is not a `Requires:` edge.

**Requires:**

- [Consuming externally-witnessed `open` and the run-loop step restructure](runloop-cps-open.md) —
  supplied the externally-witnessed sealed form and `open` the per-call child scope reads through.

**Unblocks:**

- [Production witness impls and the `alloc` witness plumbing](alloc-witness-plumbing.md) — the
  restructure gives the production bundle site a witness handle to the value's owning frame.
- [Migrate scope-handle reads to `open`](scope-reads-to-open.md) — the scope-read consolidation rides
  this restructure; it owns inverting the readers that still hand a `&Scope` back.
- [Borrow-bounded `attach` fallback](externally-witnessed-attach.md) — landed the scope-specialized
  `attach` that item generalizes (or records as the only one).
