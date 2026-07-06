# Resident hosted carriers

Migrate scopes and binding entries onto the hosted witness sets, per
[design/witness-hosting.md § Scope and bindings](../../design/witness-hosting.md#scope-and-bindings-above-the-substrate):
a resident carrier becomes `{ borrows_host: bool, reach: &WitnessSet }`, covered
by its container's liveness.

**Problem.** A binding entry owns its reach, and
[`Scope`](../../src/machine/core/scope.rs) carries the ownership apparatus
around that:

- Every carrier-oriented binding read clones the entry's stored `FrameSet` out
  per hit, on the interpreter's hottest path, because the entry owns its reach
  and the read must not hold the map borrow (the type-binding memo in
  [scope.rs](../../src/machine/core/scope.rs) caches exactly this clone).
- `Scope` holds two soundness-bearing ownership slots beside the carrier
  channel: the reach accumulator (`Scope.reach`, a `RefCell<FrameSet>` folded
  by `Scope::fold_reach` on every bind) and the deposit list (`Scope.deposit`,
  a `RefCell<Vec<CarrierPin>>` that keeps adopted carriers' severed owned
  backings alive). `Scope::fold_foreign_into` routes a delivered witness's
  `Frame` pins into the accumulator and its `Object` / `Type` pins into the
  deposit list.

**Acceptance criteria.**

- A resident carrier entry is `{ borrows_host: bool, reach: &WitnessSet }`, its
  set hosted in the host region's own arena
  ([witness-hosting.md § The carrier](../../design/witness-hosting.md#the-carrier)
  — resident locality: the reference never points into a foreign region's
  arena). Binding reads copy the thin reference — no per-hit set clone, and the
  type-binding memo no longer caches a cloned set.
- The `Scope` struct has no `reach` field and no `deposit` field. A bind mints
  the delivered value's reach into the scope's home arena through the
  [hosting substrate's](witness-set-hosting.md) verbs; the delivered carrier's
  owned backing is released once the mint has folded it — the deposit list's
  old job for `Frame` pins. A severed owned backing is re-homed at bind
  (copied into the scope's home arena), so no binding entry references a
  frame-free backing.
- The bind mint home-omits exactly what the scope already keeps alive: its own
  home frame and lexical-ancestor regions, supplied by the existing
  `Scope::chain_reaches_region` as a predicate argument to the mint. Policy
  stays in Koan; mechanism is the library verb.
- Module reach is minted once at scope close (`Scope::close`, the reach seal
  point), as the union over the child scope's binding entries' sets.
- The full Miri audit slate passes: 0 leaks, 0 UB.

**Directions.**

- *Resident form `{ bit, ref }` — decided.* Container liveness covers the
  reference (entry reachable only through scope → frame → region, and the arena
  outlives every `'a` the region hands out); no entry owns a set, no lookup
  clones one.
- *Scopes store hosted carriers — decided.* `fold_reach` /
  `fold_foreign_into` / `foreign_reach_of` become calls into the mint verbs;
  the accumulator's job (keep foreign regions alive for the scope's life) is
  done by the resident sets' members instead.
- *Module reach — decided.* Seal-time union at `Scope::close()`; no
  incremental accumulator to keep consistent.
- *Severed backings re-home at bind — decided.* A delivered carrier can carry a
  severed owned backing (`CarrierPin::Object` / `::Type` — a value with **no**
  host region), which the deposit list keeps alive today; a resident
  `{ bit, ref }` entry cannot hold it. The bind copies the severed node into
  the scope's home arena and mints its reach there: the value becomes an
  ordinary region-hosted resident, and the deposit list dies completely.
  Precedent for the boundary copy is finalize's declared-return re-stamp, which
  deep-clones the checked value into the contract's home region
  ([finalize.rs](../../src/machine/execute/finalize.rs)). Cost: one top-node
  copy per severed adoption — severed values are already copies off a dying
  frame — and a shared severed backing bound twice copies twice instead of
  sharing. A severed-only vestigial deposit slot was rejected: it would carry a
  soundness-bearing `Scope` field through two more items to save a rare copy.
  The walking-side twin is the severed variant in
  [Host-pinned walking carrier](host-pinned-walking-carrier.md).

## Dependencies

Walking carriers are untouched here: a delivered carrier still arrives as the
owned `CarrierWitness { pins, reach }` pair
([carrier_witness.rs](../../src/machine/core/carrier_witness.rs)), and the bind
mint is where it converts to the resident form. Sealing out of a scope, the
scheduler, and finalize are out of scope.

**Requires:**

- [Witness-set hosting substrate](witness-set-hosting.md) — binds mint into the
  scope's arena through its verbs.

**Unblocks:**

- [Host-pinned walking carrier](host-pinned-walking-carrier.md) — collapses the
  remaining owned form once the resident side is hosted.
