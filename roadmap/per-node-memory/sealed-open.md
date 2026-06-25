# Sealed node-storage carrier and `open`

Give the witnessed substrate its storage form — an opaque `Sealed<T, W>` read only through
a rank-2 `open` — and route the node result slot's storage through it.

**Problem.** A node's value rides a [`Witnessed<T, W>`](../../src/witnessed.rs) that exposes
construction and the rank-2 `with` directly; there is no opaque between-step form, so "this
carrier is dormant between steps" is a convention, not a type. The result slot stores its
value as a live `Witnessed` and reads it through `read_result` / `Witnessed::with`, with no
sealed boundary marking that nothing is borrowed between steps.

**Acceptance criteria.**

- A `Sealed<T, W>` node-storage form exposes `seal` (lift a `Witnessed<T, W>` into `Sealed`)
  and a single rank-2 `open<R>(&self, impl for<'b> FnOnce(Live<'b, T>) -> R) -> R`; it exposes
  no constructor or transform between accesses.
- A `compile_fail` guard pins that nothing branded `'b` escapes `open`, mirroring the shipped
  [`Witnessed::with` / `map`](../../src/witnessed.rs) guards.
- The node result slot stores `Sealed<W::Value, _>`; `read_result` / `read_result_with_frame`
  are rerouted to read through `open` internally, leaving their callers unchanged.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *`open` reuses the rank-2 brand, not a new reattach — decided.* `Live<'b, T>` is branded by
  the same `for<'b>` mechanism the shipped `with` uses; the soundness argument is already
  discharged, so this item carries no novel `unsafe`.
- *Reroute storage, not callers — decided.* This item changes only the slot's storage type and
  the `read_result` internals; converting the result-slot *callers* to `open`-only is the
  separate [value-reads-to-open](value-reads-to-open.md) item, so this stays one PR.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Externally-witnessed sealed form and `attach`](externally-witnessed-attach.md) — the
  borrow-bounded access form it adds to `Sealed`.
- [`transfer_into` and closing the lift relocation unsafe](transfer-into-lift.md) — the
  relocation verb it adds to `Sealed`.
- [Migrate result-slot value reads to `open`](value-reads-to-open.md) — the caller-side
  open-only conversion of the surface this lands.
