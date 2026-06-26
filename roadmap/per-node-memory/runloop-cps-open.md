# Consuming externally-witnessed `open` and the run-loop step restructure

Add the consuming, externally-witnessed rank-2 `open` to `Sealed`, and restructure the run-loop
step's post-continuation tail onto it so the tail's carriers re-anchor through one universal brand
instead of loose witness-borrow reattaches.

**Problem.** A node step's tail re-anchors four carriers across the step lifetime `'s`: the
continuation ([`vend_carrier`](../../src/witnessed.rs)), the pull-lifted dep slice
([`deps_at_step`](../../src/machine/execute/outcome.rs)), the `Outcome::Forward` producer value
([`reattach_with`](../../src/witnessed.rs) inside
[`apply_outcome`](../../src/machine/execute/runtime.rs)), and the return contract
([`vend_carrier`](../../src/witnessed.rs) at `Done`). Each routes a loose witness-borrow function
whose `'s` is a concrete borrow of the step cart `Rc` — a safe signature over an `unsafe` retype,
not the generative rank-2 brand the substrate's destination is. The destination `open`
([`Sealed::open`](../../src/witnessed.rs)) bundles its witness and requires `At<'static>: Copy`, so
it serves none of these: they are externally-witnessed (the cart pins them; bundling a clone pegs
the TCO `Rc::get_mut` gate) and the continuation is a non-`Copy` `Box<dyn FnOnce>`. The tail also
returns a branded [`Outcome<'s>`](../../src/machine/execute/outcome.rs) the post-step match consumes,
so confining it under a `for<'b>` brand requires the whole tail to nest inside the closure.

**Acceptance criteria.**

- `Sealed<T, W>` has a **consuming, externally-witnessed** rank-2 `open`: the witness is supplied at
  the access (not bundled), the carrier is handed **by value** into a `for<'b>` closure whose result
  `R` cannot name `'b`, and the receiver consumes the carrier (so a non-`Copy` `Box<dyn FnOnce>`
  passes). It carries its own Miri tree-borrows proof, distinct from the bundled-witness `open`.
- The run-loop step tail — the continuation run, `apply_outcome`, the step-guard exit, and the
  `Done`/`Replace`/`Alias` realization — runs inside one rank-2 brand standing in for `'s`; the
  `Outcome` and finalized `Carried` it produces are consumed in place (erased into the slot store
  before return), and nothing branded escapes the closure.
- The four run-loop-tail re-anchors (the two `vend_carrier`, the `apply_outcome` `Outcome::Forward`
  `reattach_with`, the `deps_at_step` slice) route the consuming `open` and are deleted; the tail
  carries no loose witness-borrow reattach of its own.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *The unified witness is the singleton start cart — decided.* A `for<'b>` brand cannot be satisfied
  by a separate concrete witness borrow for a secondary carrier, so every tail carrier re-anchors
  through one witness supplied at the brand. A spike proved that witness is the **start cart** alone:
  it pins the continuation, deps, and Forward value directly, and its `outer` chain subsumes the
  return contract's home (a strict ancestor of the producer frame, see
  [`finalize.rs`](../../src/machine/execute/finalize.rs)) — so the contract re-anchors against the
  start cart, not the end-of-step frame. The Miri slate confirmed the subsumption (0 UB, 0 leaks).
- *Consuming externally-witnessed `open`, not borrow-bounded `attach` — decided.* The tail nests
  every consumption inside the brand, so no reference escapes up-stack; the rank-2 `open` (generative
  brand, un-widenable) is the verb, and the borrow-bounded [`attach`](externally-witnessed-attach.md)
  is not needed here.
- *Does not require the set-witness unification — decided.* The value/continuation path witnesses
  uniformly with `Rc<CallFrame>`, so the witness "set" is a singleton, not a union; the
  `Rc<CallFrame>`-vs-`Rc<FrameStorage>` unification ([alloc-witness-plumbing](alloc-witness-plumbing.md))
  is only needed where a value-carrier `merge`s a scope-carrier, which this restructure never does.
- *Whether `apply_outcome` / `finalize_terminal` fold into the brand as one `open` or as merged
  operands — open.* The `Outcome::Forward` re-anchor and the contract vend both move inside the
  brand; whether they ride a single multi-carrier open or are merged in is settled during
  implementation.

## Dependencies

This is the keystone a spike de-risked: the `*-reads-to-open` items' shared substance is this
restructure plus the `open` verb it adds.

**Requires:** none — builds on the shipped `Sealed` / `open` substrate; spike-proven feasible and
sound.

**Unblocks:**

- [Borrow-bounded `attach` fallback](externally-witnessed-attach.md) — supplies the
  externally-witnessed sealed form the contingent `attach` would back up.
- [FrameStorage self-reference removal](framestorage-self-reference.md) — the per-call child scope
  reads through this `open`.
- [Migrate the loose witness-borrow wrappers onto `Sealed`](migrate-reattach-helpers.md) — the
  remaining reattach sites route this verb.
- [Migrate result-slot value reads to `open`](value-reads-to-open.md) — the value reads ride this
  restructure shape.
- [Migrate scope-handle reads to `open`](scope-reads-to-open.md) — the scope reads ride it.
