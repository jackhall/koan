# Give in-flight binder claims their own store

**Problem.** A still-finalizing binder's claim is stored as an extra arm of the
very binding-table slot it will resolve into: `data[name]` is
[`ValueSlot`](../../src/machine/core/bindings.rs) `Bound` xor `Pending`,
`types[name]` is `TypeSlot` `Bound` / `Pending` / `BoundWithPending`, and
`functions[key]` is a `Vec` of `OverloadSlot` mixing `Sealed` and `Pending`.
Claims and bindings have opposite shapes — a claim is transient, block-local,
never crosses a scope view, and is addressed by its producer; a binding is
durable, per-scope, name-keyed, and copied into module views — and storing the
first inside the second forces every reader and writer to carry the distinction:

- Retirement (`KoanWorkload::retiring`,
  [`harness.rs`](../../src/machine/execute/harness.rs)) must find a slot's claims
  by producer in tables keyed by name, so
  [`clear_placeholders_for_producers`](../../src/machine/core/bindings.rs)
  `retain`s over `data`, `types`, and every `functions` bucket, testing each
  pending slot against the retiring slot's owned-edge list. That is a full
  three-table walk per retiring slot that owns any edge — quadratic in the number
  of binders in a block — and it runs on the success path, where the commit has
  already overwritten the claim and the walk is guaranteed to find nothing. A
  bare-name forward's classification edge (`harness.rs`) shares the same owned
  list, so a `Forward` slot pays the walk for an edge that can never match.
- `write_value` and `write_type` each need a "a pending slot does not block"
  clause in their cross-kind exclusion probes, and each carries in-place-finalize
  logic to overwrite a claim where it sits.
- `bulk_install_from` filters pending arms out of every table it copies, because
  a claim names an edge of its own run.
- `lookup_function`'s bucket read builds the visible sealed overloads even for a
  caller that only wants the pending arm, which is why the pending-only peer
  beside it exists at all.

**Acceptance criteria.**

- The binding tables hold committed bindings only: `data[name]` is a bound entry,
  `types[name]` is a bound identity, and `functions[key]` holds sealed overloads.
  `TypeSlot::BoundWithPending` has no counterpart — a nominal's seal
  pre-installing an identity while its binder is in flight is a bound entry plus
  a live claim.
- A scope's in-flight claims live in a store inside
  [`Bindings`](../../src/machine/core/bindings.rs) with three parts: `by_name`
  and `by_bucket` answering the read path in one hash probe each, and a
  `by_statement` run, sized at the block fan-out and indexed by `BindingIndex`,
  carrying each statement's claimed keys and a live mask over them.
- A commit retires its own claim: `write_value`, `write_overload` and
  `write_type` remove the claim for the name (or bucket key) and `BindingIndex`
  they are writing, in constant time, with nothing searched.
- `KoanWorkload::retiring` indexes `by_statement` by the retiring slot's own
  `BindingIndex` and returns on a zero live mask; a non-zero mask removes the
  named keys directly. No path scans a binding table, and no door takes a
  `ProducerId` membership predicate.
- Retirement still releases every edge the slot owns, exactly once, on every
  terminal and on the alias splice.
- A debug assertion fires if a scope is fanned out into more than once, and one
  fires if a slot owning claims reaches a tail replace.
- The statement-at-a-time submission door
  ([`dispatch_in_scope`](../../src/machine/execute/harness.rs)) builds no claim
  store, and a driver using it resolves earlier committed bindings normally.
- `bulk_install_from` and the `iter_*` readers copy the binding tables with no
  pending-arm filter.
- A test asserts that a binder whose body errors before its write path leaves no
  claim behind, and that a sibling parked on it wakes to `UnboundName`.

**Directions.**

- *Where the store lives — decided.* Inside `Bindings`, not beside it on `Scope`.
  A consumer may park on an in-flight binder in an outer scope, and the
  resolution walk probes each ancestor's `bindings` gated by that scope's cutoff
  ([`resolve.rs`](../../src/machine/core/scope/resolve.rs)), so a store anywhere
  else is invisible to the ancestor probe.
- *What retirement keys on — decided.* The retiring slot's own `BindingIndex`,
  which is core currency and the one address the slot knows about itself. Keying
  on the producer is what forces the search; keying on the name is what forces it
  in the other direction.
- *Read-path shape — decided.* Hash probe, not a scan over the statement run. The
  claim probe sits on the resolution walk's miss path, once per ancestor scope.
- *The live mask — decided.* Keep it. It makes success-path retirement one array
  index and a zero test rather than up to three hash removals that all miss, and
  the success path is the common one.
- *One `by_name` map for both name channels — decided.*
  [`partition_guard`](../../src/machine/core/bindings.rs) decides Value from Type
  by token class alone, so value and type claims cannot collide.
- *Claim edges — decided.* Still minted at submission and owned by the slot.
  `Scheduler::install_edge` requires a live producer and nodes are reclaimed at
  finalize, so a claim cannot hold a node identity and mint lazily; `NodeId` and
  `EdgeId` stay out of `machine/core` regardless.
- *Duplicate-name attribution — decided.* Two binders claiming one name in a
  block are detected when the store is built at fan-out, and the error names both
  declaring statements rather than being attributed to whichever slot installs
  second.
- *Fan-out multiplicity — decided.* Assert single-fan-out-per-scope and size
  `by_statement` as a fixed run. No shipped form fans out into a scope twice, and
  a spliced binder under
  [EVAL splices in place](../metaprogramming/eval-splices-in-place.md) needs no
  claim: the barrier already gives every later sibling a dep edge on it.

## Dependencies

**Requires:** none — the claim store is self-contained in `machine/core` plus the
submission and retirement sites.

**Unblocks:** none.
