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
- A declared-return mismatch and a `finalize_error` frame each identify the callable two
  ways at every one of the three reader sites: the frame's `function` field carries the
  **call site's source text** (the invoked expression, resolved from its span at render
  time — a keyword call's `BOOM 1` and a bound-value call's `f {x = 7}` alike), and the frame's
  `expression` field carries the by-name identity — the function's `value_ktype` rendered
  through `KType::name`, `:(FN (x :Number) -> Number)` — with the frame's location
  resolved from the same span. A contract sealed from an expression with no source extent
  falls back to the by-name render alone.
- The string `PRINT` renders and returns for a function value is that same by-name
  identity, and `KFunction::summarize` is deleted.
- The recorded tail-loop baseline in [audit/README.md](../../audit/README.md) drops by the
  deferred share, and `tests/allocation_baseline.rs` is re-measured to the new figure.

**Directions.**

- *Callable render source — decided.* The obligation carries two `Copy` handles: the
  callable's `value_ktype` — the interned `TypeNode::KFunction` handle
  ([`KFunction::value_ktype`](../../src/machine/core/kfunction.rs)), rendered through the
  existing `KType::name` — and the **call site's span + file**, rendered to the invoked
  expression's source text. The by-name identity (the parameter record keyed by declared
  names, keywords excluded) is the right render for the function *value*: the anonymous
  form registers no keyword and is reachable only through the name a `LET` binds, which
  is call-site knowledge the value cannot hold. So the callable's *which-one* identity is
  conveyed separately, by the call-site text — already minted as `Copy` span data beside
  every contract construction site (the `WorkLabel` mint in
  [src/machine/execute/decide/exec.rs](../../src/machine/execute/decide/exec.rs)), and
  every parse path registers its source (`<input>` when pathless), so the span resolves.
  Both handles are `Copy` and lifetime-free, satisfying the obligation's lifetime
  constraint. Deleting `KFunction::summarize` needs its other caller routed too — the
  bullet below.
- *`KObject::summarize`'s function arm — decided.* `KFunction::summarize`'s other caller is
  the `KObject::KFunction(f)` arm of `KObject::summarize`
  ([src/machine/model/values/kobject.rs](../../src/machine/model/values/kobject.rs)) — the
  path `PRINT` renders a function value through. It renders the callable's `value_ktype`
  through `KType::name` as well, the same by-name identity the error arms take, so
  `summarize` loses its last caller and is deleted here. This moves what `PRINT (f)`
  yields — `:(FN (x :Number) -> Number)` where it rendered `fn(DOUBLE <x>)` — and `PRINT`
  returns what it renders, so the move is observable to a program, not only to a reader.
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
- *Frame currency — decided.* The deferred label is a new `DeferredTraceFrame` arm
  carrying the call site and the `value_ktype`, reusing the retained-frame currency the
  dep-error channel already runs on. The considered alternative — a label-shaped type
  private to `ReturnObligation` — rested on the third reader spending the label as a
  message fragment, which it does not: all three readers spend it only as a
  `TraceFrame`, so one render path serves them all.

## Dependencies

**Requires:** none — the render source is the already-shipped `value_ktype` path.

**Unblocks:** none.
