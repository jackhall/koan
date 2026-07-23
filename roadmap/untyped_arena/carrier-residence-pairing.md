# Compile-enforced carrier residence pairing

Make "a delivery envelope's host pins the value's residence region" a construction
invariant, so host drift — and the spurious residence-audit rejection it causes — is
unrepresentable rather than caught at runtime.

**Problem.** A carrier encodes residence *home-relatively*: its `borrows_host` bit means
"borrows the region my delivery envelope's host pins"
([carrier_witness.rs](../../src/machine/core/carrier_witness.rs), `Delivered` in the
`workgraph` crate), and a reach set names only foreign members — never the value's own home,
which would be the self-cycle `RegionSet::mint` forbids. This is complete only while the
envelope host pins the value's residence region. Nothing enforces the pairing:
`Delivered::hosted` accepts any `(cell, host)` pair — "by construction" is convention, not a
type fact. The relocation verbs
([finalize.rs](../../src/machine/execute/finalize.rs),
[runtime.rs](../../src/machine/execute/runtime.rs)) re-home a value into the transfer
destination, after which the scheduler's `dep_delivered` re-pairs the *producer frame's*
retention hold as host. The bit's referent silently drifts to a region the value no longer
borrows, and the value's true residence region is named nowhere.

The only guard is a runtime residence audit
([`Scope::alloc_object_delivered`](../../src/machine/core/arena/residence.rs) →
`Residence::owns_substrate`), and it spuriously rejects valid programs. Chaining a
substrate-returning operator three deep through a binding —

```
LET r1 = {a = 1}  LET r2 = {a = 2}  LET r3 = {a = 3}
MODULE recs = ((OP #(&) OVER :{a :Number} = (right)) (LET chained = (r1 & r2 & r3)))
```

— errors with `borrows a region not covered by dest, the supplied evidence, or the
destination scope's ambient coverage`: the drifted reach names a dead producer frame while
the true home region goes unnamed, and the audit's substrate arm cannot consume the ambient
coverage that *is* present (a substrate is a raw pointer whose home region is unrecoverable).
A runtime check stands in for an invariant the type system could enforce.

**Acceptance criteria.**

- The pairing "a delivery envelope's host pins its value's residence region" holds by
  construction: an envelope carrying a host that does not pin the value's residence has no
  constructor to call, so host drift is unrepresentable — a type-shape fact, not a
  borrow-checker one.
- A substrate-returning operator chained three or more deep through a binding
  (`r1 & r2 & r3` under an `OP` binding, whether in a `MODULE` body or a `USING … SCOPE`)
  evaluates without a residence-audit rejection.
- The runtime residence audit is defense-in-depth behind the typed pairing, not the sole
  soundness line — retired where the pairing subsumes it, or documented as a redundant
  backstop.
- The Miri audit slate is green with the re-homed-carrier pairing exercised.

**Directions.**

- *Residence-owner channel — open.* Every envelope built from a relocated carrier must carry
  the value's residence owner (its home `Rc<FrameStorage>`), captured from the transfer
  destination the caller already holds. Whether this needs a new owner-paired terminal type
  (an `Rehomed<P, F>` bundling `Witnessed<P, Carrier<F>>` with its residence `Rc<F>`) or the
  existing `Witnessed` / `Delivered` carriers already thread enough to recover the residence
  owner is open — the new type may not be needed; the existing carriers likely already carry
  it. Recommended: reuse the existing `Witnessed` carriers (retype the existing terminal
  channel rather than add a parallel field).
- *Runtime audit disposition — open.* Keep `Residence::owns_substrate`'s audit as
  defense-in-depth, or retire it once the pairing is enforced by construction.
- *Residence lifetime — decided.* Not a lifetime or brand: carriers are stored `'static`
  (erased and re-anchored under rank-2 brands per step), so residence cannot be a
  borrow-checker-tracked lifetime; the pairing rides an external owner pin, and the
  home-relative bit encoding stays (naming the home in the reach is the forbidden self-cycle).

## Dependencies

**Requires:** none — a correctness fix to existing carrier machinery.

**Unblocks:**

- [Region evacuation at frame death](region-evacuation.md) — its all-carrier bind-seam
  pricing needs drift-free residence so pricing every carrier does not hit spurious rejects.
