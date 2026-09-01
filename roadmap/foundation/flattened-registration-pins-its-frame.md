# A flattened dispatch registration pins its defining frame

Teach the environment copy
([lazy-closures.md § Lazy close](../../design/lazy-closures.md#lazy-close-the-copy-verb-through-callables))
to consolidate a registration whose callable captures a frame that is still
open at the crossing, so a `CLOSE` block that flattened one stops holding its
producer chain.

**Problem.** Implicit close copies **every** dispatch registration in the
per-call portion of the enclosing chain into the block scope
([`close_over.rs`](../../src/builtins/close_over.rs)), pinned with its full
transitive reach. A registration's callable captures the scope that declared
it — a per-call frame that is still **open** at the moment the block evaluates,
since the block runs inside it. The copy engine rebuilds a captured chain only
when every link passes the readiness gate
([`Scope::is_copy_ready`](../../src/machine/core/scope.rs)), whose first clause
is `is_closed()`, so the nested rebuild declines and the registration rides
verbatim ([`rebuild_in_fold`](../../src/machine/core/scope/copy.rs)) — sound,
but it keeps the producer frame's region alive for as long as the escaped
closure holds the block.

The block's own scope consolidates: its data bindings, its nested callables and
its operator registry are all rebuilt at the destination. What survives is the
frame the flattened registration names, and the count grows by one region per
enclosing producer frame, against the `(1, 0)` an anonymous-`FN` producer chain
answers
([close_over/tests/consolidation.rs](../../src/builtins/close_over/tests/consolidation.rs)).

An `OP` declaration inherits this whole, because it is inseparably also a
dispatch registration: each `OP` writes both the enclosing scope's function
bucket and its operator registry
([`op_def.rs`](../../src/builtins/op_def.rs)). The registry half is rebuilt by
the copy engine, so a flattened operator now retains exactly what its own
keyworded registration retains and no more
(`a_flattened_per_call_operator_retains_no_more_than_its_registration` in
[close_over/tests.rs](../../src/builtins/close_over/tests.rs)) — this item owns
that remaining registration cost, for operators and plain keyworded `FN`s
alike.

The retention is currently deliberate, and this item is the decision to revisit
it: [close_over/tests.rs](../../src/builtins/close_over/tests.rs)'s module
header records that a keyworded `FN` in a per-call frame is pinned on purpose,
because the flatten had no way to sever it.

**Acceptance criteria.**

- A closure escaping a `CLOSE` block that flattened a per-call dispatch
  registration holds **one** region and releases everything else, at every
  producer-chain depth — the `(1, 0)` the anonymous-chain census already
  asserts.
- The flattened registration's callable dispatches identically after every
  producer frame has died, for a keyworded `FN` and for an `OP` chain alike.
- The engine states in one predicate why an open defining frame is or is not
  rebuildable at the flatten, rather than the flatten silently inheriting the
  general readiness gate's answer.

**Directions.**

- *Copy the registration against the block's own copy — open.* The flatten
  already knows the block scope it is installing into; rebuilding the callable
  against that scope, rather than against a copy of its open defining frame,
  severs without ever needing the frame to be closed. Bounds the change to what
  the block actually reached.
- *Snapshot an open frame — open.* Let the copy rebuild an open link from the
  bindings committed at the crossing. Sound only if a later bind in that frame
  can never be reachable from the escaped closure, which is a claim about the
  flatten's timing, not about the gate.
- *Narrow the flatten — open.* Copy only the registrations the block actually
  dispatches, per the free-identifier walk
  ([`close_inference.rs`](../../src/machine/model/close_inference.rs)). Shrinks
  the problem without closing it — a block that genuinely calls a per-call
  registration still pins — so it is a complement, not a substitute.

## Dependencies

The sibling item [Callable copy tuning](callable-copy-tuning.md) owns the
crossing-time levers, including the foreign-crossing rule that is why a
registration pinned here is never re-consolidated at a later escape; this item
owns the flatten-time decline that creates the pin in the first place.

**Requires:** none — the copy engine it extends is shipped.

**Unblocks:** none tracked yet.
