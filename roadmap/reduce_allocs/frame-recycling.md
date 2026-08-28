# Frame recycling

**Problem.** Every frame retirement releases a set of heap allocations the next mint
immediately re-pays. A steady `FreshTail` tail hop mints one whole cart — `Rc<FrameStorage>`,
`Rc<CallFrame>` shell, region chunk, envelope — plus its `Rc<SlotFrame>` anchor and two
`LexicalFrame` head links; the wide composite body pays the general form of the same term
per submission: `SlotFrame::new` 61/step (plus 18/step via `replacing` / `opening`),
`Rc<LexicalFrame>` 30/step, a `CallFrame` with region host and first chunk 21/step
([observe/alloc.txt](../../observe/alloc.txt); shares from the 2026-08-27 dhat sweep of the
wide pair).

TCO originally recycled tail carts as a two-generation ping-pong (`FramePlacement::ReuseReserve`,
`active_reserve` rotation, `CallFrame::try_reset_for_tail`), deleted by the retired
`tco-library-region-reuse` item (commits `b8e898a4`, `480b3a15`) because it put region
lifetime in Koan's hands, which the library-owned-regions boundary
([design/scheduler-library.md](../../design/scheduler-library.md)) forbids. That ruling
stands: recycling returns as a **library-side facility of the node lifecycle**, with Koan
seeing the same `FreshTail` / `FreshChild` placements it sees today. The deletion's Miri
run also surfaced a Tree Borrows UB sitting in the old reset path's test — the new path's
slate coverage is not optional.

**Acceptance criteria.**

- A steady tail loop's hop draws its cart from the pool: steady state mints no fresh
  `Rc<FrameStorage>`, `Rc<CallFrame>`, region chunk, `Rc<SlotFrame>`, or chain-head
  allocation, and the allocation-baseline shapes rebaseline accordingly.
- The general case recycles too: frames and anchors retiring through `Done` feed the
  same pool, and the `wide_step` anchor/chain/frame terms drop measurably.
- A pinned frame (escaping closure, shared cart) falls back to a fresh mint; recycling
  is semantically invisible — observable behavior is identical with the pool disabled.
- Pool, eligibility gate, and reset live behind `NodeStore`; Koan-side code carries no
  recycle source across steps and decides nothing about region lifetime
  ([design/scheduler-library.md](../../design/scheduler-library.md) boundary preserved).
- Reset-at-retire releases the retiring region's retained pins at retirement, not at draw.
- A recycled frame's scope carries a fresh generative `ScopeId`.
- No reachable reference into reset memory exists during pool residency, enforced by
  type rather than by audit.
- The Miri audit slate exercises the recycle path, the escape-gate fallback, and pool
  residency.

**Directions.**

- *Pool — decided (2026-08-27 design discussion).* A free list of retired frames on `NodeStore`
  ([workgraph/src/scheduler/node_store.rs](../../workgraph/src/scheduler/node_store.rs)) —
  scheduler-private, so eligibility, reset, and capacity policy are invisible to Koan.
  The pooled unit is the whole retired `Rc<W::Frame>` anchor chain (anchor, cart shell,
  storage), not bare storage.
- *Retire — decided.* The displaced anchor already lands in exactly one library-side spot — the
  drain's `Replace` arm and `finalize` on `Done`
  ([workgraph/src/scheduler/drain.rs](../../workgraph/src/scheduler/drain.rs)). There the
  library gates on unique hold (`Rc::get_mut` succeeding for anchor, shell, and storage —
  an escapee's storage clone or a Yoked slot's shared cart fails the gate and drops
  normally), resets the region in place (bump reset keeping the chunk, the inline reach
  cells, the spill table, and **dropping the retained pin bundle immediately** — pooled
  storage must not keep dead regions alive), and pushes. Reset happens at retire, never
  deferred to draw.
- *Draw — decided.* The destination region must exist at decide time: argument adoption into a
  fresh cart runs before the replace, while the retiring region is still the deciding
  step's own ([design/tail-call-optimization.md § Soundness](../../design/tail-call-optimization.md#soundness),
  `enter_user_fn` in [src/machine/execute/decide/exec.rs](../../src/machine/execute/decide/exec.rs)).
  So the library provisions **at step entry**: the drain's per-step `Step` hand-out grows
  an opaque provisioning capability, scoped to the step borrow, and the decide draws its
  cart through it (both mint sites — the FN invoke and EVAL's `FreshChild`, whose `BodyCtx`
  rides within the step). Koan holds no storage across steps and makes no lifetime,
  eligibility, or timing decision — what distinguishes this from the deleted
  `active_reserve` apparatus.
- *Rebuild — decided.* A door consumes the pooled unit plus the new outer scope and produces a
  fresh frame in one act: re-minted child scope and envelope, a **fresh generative
  `ScopeId`** (ids are part of generative type identity and are never reused), and the
  uniquely-held `Rc<LexicalFrame>` head links and anchor shell rewritten in place under
  the same unique-hold gate. `LexicalFrame` keeps its generative key: a static block-id
  key has a cross-invocation counterexample (an escaped closure called during another
  live invocation of its defining block would import that invocation's cutoff), so the
  chain's cost dies by recycling its allocations, not by re-keying its identity.
- *Type guard — decided.* After reset, the pooled shell's envelope holds an erased scope
  reference into reset memory. The pooled form is therefore a distinct type whose only
  exit is the rebuild door, so the dangling erased reference is unreachable by type
  during pool residency; if Miri objects to merely storing it, the fallback is moving
  `Erased`'s internal representation to a raw pointer (library-internal). No new
  `unsafe` is expected on either path.
- *Yoked anchor shells — open.* A `Done`-retiring sub-expression slot holds its cart
  shared, so the whole-unit gate rejects it; whether anchor shells pool separately from
  full carts (a two-tier pool) or only whole-unit retirements recycle is an
  implementation trade to price when the dhat shares are fresh.
- *Pool capacity — decided.* Bounded, with the policy library-internal to `NodeStore`.

## Dependencies

**Requires:**


**Unblocks:** none tracked yet.
