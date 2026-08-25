# Deferred return-obligation labels

**Problem.** Every call renders its own trace label whether or not it ever errors.
`ReturnObligation::seal`
([src/machine/execute/obligation.rs](../../src/machine/execute/obligation.rs)) resolves a
`ReturnContract` into a dormant obligation and renders `label: String` eagerly: a
`Function` or `PerCall` contract opens the callable and calls `KFunction::summarize`
([src/machine/core/kfunction.rs](../../src/machine/core/kfunction.rs)), a `Vec<String>` of
signature elements joined through `format!`; an `Arm` contract calls `to_string()` on a
`kind` that is already a `&'static str`
([src/machine/core/kfunction/body.rs](../../src/machine/core/kfunction/body.rs)).
`ReturnObligation::duplicate` then clones that `String` at each of the eight
`current_obligation_duplicate` sites a tail chain passes through, so the label is
re-allocated per hop as well as per call. All three readers are error arms —
`finalize_error` and `return_type_mismatch`
([src/machine/execute/finalize.rs](../../src/machine/execute/finalize.rs)) both spend the
label immediately as `TraceFrame::bare(label, label)`, and the `Disposition::Mismatch`
reader only runs once a returned value has failed its declared type. The label is
rendered on the success path and read only on the failure path — the same pattern
[audit/README.md](../../audit/README.md)'s recorded per-step total already attributes to
`ReturnObligation::duplicate` at ≈8 allocations per tail-loop step, where the clone is the
function's only allocation.

**Acceptance criteria.**

- A call that returns without a declared-return violation renders no label text: the
  seal-time `summarize` and the per-hop label clone are gone.
- `ReturnObligation` is `Copy`, and `duplicate` is deleted in favour of it.
- A declared-return mismatch and a `finalize_error` frame each name the callable by its
  by-name identity — the function's `value_ktype` rendered through `KType::name`,
  `:(FN (x :Number) -> Number)` — at every one of the three reader sites.
- The recorded tail-loop baseline in [audit/README.md](../../audit/README.md) drops by the
  deferred share, and `tests/allocation_baseline.rs` is re-measured to the new figure.

**Directions.**

- *Callable render source — decided.* The obligation carries the callable's
  `value_ktype` — the interned `TypeNode::KFunction` handle
  ([`KFunction::value_ktype`](../../src/machine/core/kfunction.rs)) — and the error arms
  render it through the existing `KType::name`. That is the *by-name* identity: the
  parameter record keyed by declared names, keywords excluded — the right identity for a
  label naming a callable that was invoked, and already rendered by every by-name
  diagnostic. A `KType` is `Copy` and lifetime-free, so it satisfies the obligation's
  lifetime constraint with no new render machinery, and `KFunction::summarize` loses its
  last caller and is deleted here.
- *Keeping the obligation lifetime-free — decided.* `ReturnObligation` deliberately names
  no region, so the tail chain carries nothing region-bound and no path downstream reopens
  a live contract. A deferred label must preserve that: whatever handle replaces the
  `String` is `Copy` and lifetime-free. Holding the contract's `SealedFunction<'home>`
  directly would thread `'home` back through the obligation and every continuation that
  captures it, which is what makes the `Copy` `value_ktype` handle the one that composes
  here.
- *The `Arm` label — decided.* `ReturnContract::Arm`'s `kind` is already a `&'static str`,
  so its `to_string()` drops outright with no render deferral needed. Independent of the
  callable fork and the cheapest part of the item.
- *Frame currency — open.* Whether the deferred label becomes a new
  `DeferredTraceFrame` arm — reusing the retained-frame currency the dep-error channel
  already runs on, so `finalize`'s two frame sites render through one path — or a
  label-shaped type private to `ReturnObligation`, since the third reader spends the label
  as a message fragment rather than as a frame. Recommended: a `DeferredTraceFrame` arm,
  with the message reader reading through the same handle.

## Dependencies

**Requires:** none — the render source is the already-shipped `value_ktype` path.

**Unblocks:** none.
