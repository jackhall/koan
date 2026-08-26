# Frame and scope bookkeeping

**Problem.** The dhat profile of the audit shapes attributes the bulk of the tail-loop
step term to per-hop frame and scope construction. A steady hop mints **two** whole
carts: the `FreshTail` cart the call itself installs, and a second one `arm_tail`
([src/builtins/branch_walk.rs](../../src/builtins/branch_walk.rs)) mints unconditionally
so the selected MATCH / TRY arm has somewhere to bind `it`, whether or not the arm reads
it. Each cart is a `CallFrame::new`
([src/machine/core/arena/frame.rs](../../src/machine/core/arena/frame.rs), 4/step across
the pair) — a fresh `Rc<FrameStorage>` and the `Rc<CallFrame>` shell — plus
`build_frame_child_witnessed` (6/step), which mints the cart's region and bumps the child
scope and its delivery envelope into it, plus one region chunk each. The slot layer
allocates alongside: every tail replace mints a fresh `Rc<SlotFrame>`
([src/machine/execute/nodes.rs](../../src/machine/execute/nodes.rs)), and
`LexicalFrame::push` ([src/machine/core/lexical_frame.rs](../../src/machine/core/lexical_frame.rs))
mints an `Rc` per pushed chain link and `assemble_body_chain` stages its hits in a heap
`Vec`, even though the suffix a steady hop rebuilds is content-identical to a sub-chain
the retiring slot's chain already holds. The largest single site is the binding seam:
`Scope::adopt_carried`
([src/machine/core/scope/reach.rs](../../src/machine/core/scope/reach.rs), 9/step)
re-homes each delivered value into the consuming scope. All of it is build-use-drop
within one hop — the loop's steady state churns through allocations whose lifetime is
exactly one iteration.

**Acceptance criteria.**

- MATCH / TRY arms run frameless: a re-profile of
  `audit/shapes/tail_loop_steps100.koan` attributes no per-step `CallFrame::new`,
  `build_frame_child_witnessed`, or region-chunk term to `arm_tail` — a steady tail hop
  mints one cart and one chunk (the `FreshTail` call's own), not two.
- A steady hop mints chain links only for the scopes it opens: `assemble_body_chain`
  shares the standing chain's matching suffix by `Rc` clone and stages no heap
  container, so the re-profile attributes no per-step `Vec` term to it and the
  surviving per-step `LexicalFrame::push` count is exactly the two block heads naming
  the hop's own fresh scopes — the FN-body head and the arm overlay's head, recorded
  under [Unplanned work](README.md#unplanned-work) beside the cart mints they would
  recycle with.
- `Scope::adopt_carried`'s nine per-step allocations are attributed site-by-site, and
  every one not required by a genuine cross-hop escape is removed — transients move to
  the step scratch arena, and the `Pin` arm's bookkeeping allocates nothing.
- The `step`, `leading_loop`, and `try_loop` terms in `observe/alloc/terms.txt` drop by
  the removed share, and the affected bounds in `tests/allocation_baseline.rs` are
  re-measured through the pre-commit rebaseline so the record and the change land in one
  commit.

**Directions.**

- *Arm overlay — decided.* The selected arm runs in the enclosing cart under
  `FramePlacement::Inherit` with a `BlockScope::Overlay`, the tier `USING` already runs
  on ([src/builtins/using_scope.rs](../../src/builtins/using_scope.rs)): the overlay is a
  plain same-region child (`Scope::alloc_child_under`, bump-allocated, zero heap), and
  `arm_tail`'s seed binds `it` into it unchanged — `bind_delivered_direct` is
  scope-general, and `block_tail`'s Overlay arm mints the same unpublished-scope
  `WriteGate` the frame arm does. Nested arms stack overlays, so `it` shadowing falls out
  of the ancestor walk, and the arm's terminal is already home in the enclosing region,
  so the Done-boundary lift out of a dying arm frame disappears with the frame. `ChainOp`
  needs no sorting: `ChainOp::decide` routes only `Function`/`PerCall` contracts to
  `AssembleBody`, so an `Arm`-contract tail already takes `PushBlock`, which names the
  overlay's scope for the arm's statement cutoffs.
- *Chain reuse — decided.* Suffix sharing inside `assemble_body_chain`: build the
  parent chain by recursion over the body scope's lexical `outer` walk (depth is
  source-level nesting, so the heap `Vec` of hits goes away — the recursive-visitor
  precedent from the dispatch-resolution item), and at each hit share the call-site
  chain's own sub-chain `Rc` when its head names the same `(scope_id, index)` and its
  parent is pointer-equal to the chain just built. Zero semantic change — the output is
  structurally identical, only `Rc` identity is shared — and the steady state shares
  the whole suffix every hop. A *head* gate cannot fire and is not attempted:
  `ScopeId`s are generative (minted per scope instance), so the fresh cart scope's and
  fresh overlay's head links never repeat across hops; those two per-hop mints are this
  identity model's floor and sit with the cart mints under
  [Unplanned work](README.md#unplanned-work).
- *The adoption seam — decided.* Attribution first — `dhat --detail` on the
  tail-loop pair names the nine sites — then the dispatch-resolution precedent applies:
  transient compositions move to the step scratch arena, and only a genuine cross-hop
  escape keeps an allocation. The dhat run sizes the win; it does not gate the direction.
- *Cart recycling — deferred.* Out of scope for this item. The `FreshTail` cart's own mints (and the
  per-hop `Rc<SlotFrame>` anchor, which would ride the same mechanism) stay after this
  item. Reusing the hop-before-last's storage was TCO's original design and was
  deliberately deleted by the `tco-library-region-reuse` item for violating the
  library-owned-regions boundary in
  [design/scheduler-library.md](../../design/scheduler-library.md); any revival must be a
  workgraph-side facility of the node lifecycle, never Koan-side reserve plumbing. The
  full history, the pin fallback, and the boundary constraint are recorded in
  `scratch/tail-cart-recycling.md`.

## Dependencies

**Requires:** none.

**Unblocks:** none.
