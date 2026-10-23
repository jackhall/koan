# A birth solved once

A callable's birth facts and a call's solution, each computed once from what
the caller already holds.

**Problem.** A callable's birth, at the load and in an operator run, derives
the same answer more than once:

- **A call by name keyed on a `FOR ALL` name.** `seek`
  ([`specialize.rs`](../../src/program/specialize.rs)) runs `admitted` over
  the arguments to solve the group whose solution the specialization key
  digests, and the frame's birth in [`body.rs`](../../src/program/body.rs)
  runs `admitted` again over the same arguments, since `KBirth::Call`
  ([`bundle.rs`](../../src/program/bundle.rs)) carries `contributed` but no
  solution.
- **A frame's keyed route.** `called` ([`evaluate.rs`](../../src/dispatch/evaluate.rs))
  reads `specialization_keys` in its `keyed()` check and `seek` reads it again
  for the same callee.
- **A body's caching bit.** `caches(shape)` is re-read from the body's folds in
  `run`, in `woken`, per yielding unit in `perform`, and in `ended`
  ([`body.rs`](../../src/program/body.rs)), though it is fixed for the
  runner's body.
- **A callable's birth facts at the load.** `birth`
  ([`walk/types.rs`](../../src/dispatch/walk/types.rs)) writes `Born` with an
  empty `digestible` through `set_born`, then asks `digests` per parameter,
  which reads the callable back through `body_type` → `born` from the store,
  re-runs `in_body_of` — instantiating the group and interning the result —
  and two `read_through`s per parameter, and then writes `Born` a second time.

**Acceptance criteria.**

- A call by name keyed on a `FOR ALL` name solves its group once between the
  asker and the frame's birth.
- One call reads a callee's specialization keys once.
- A runner reads its body's caching bit once.
- A callable's birth writes `Born` once and instantiates its type once,
  deciding each parameter's digestibility from the callable type in hand.
- A test counts `admitted` calls over a keyed call by name and counts one.

**Directions.**

- *Carrying the solution — open.* `KBirth::Call` gains the solved group, set
  by `seek` and read by the birth in place of `admitted`; or `seek` returns
  it inside the `Seek` the drain hands back to the frame's birth; or the
  build arena's frame request, which carries the binding's types as an
  image, carries the solution with them. The first two touch `KBirth` and
  `Seek` before the arena rewrites both. Recommended: the request's image.
- *The caching bit — open.* A `bool` on `Runner`, set when the runner is
  built; or a field of the activation, which every runner of a body shares.
  Recommended: on `Runner`.

## Dependencies

[The build arena](build-arena.md) replaces `seek`, the route `called` decides,
and the request a missed specialization's frame is born from.

**Requires:**

- [The build arena](build-arena.md) — its frame request and recorded route
  are where the solution and the keys are read once.

**Unblocks:** none.
