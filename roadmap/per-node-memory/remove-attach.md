# Remove `attach`

Delete the transitional `attach` accessor once every consumer is on `open`, leaving `Sealed` with
a single access verb.

**Problem.** [`attach`](externally-witnessed-attach.md) exists:
[framestorage-self-reference](framestorage-self-reference.md) landed a scope-specialized
`SealedExtern<ScopeRefFamily>::attach` for the frame's un-nestable child-scope readers, and
[externally-witnessed-attach](externally-witnessed-attach.md) may generalize it to a `Sealed<T>` verb.
It is the transitional borrow-bounded accessor that lets a re-anchored reference ride up the
dispatcher call stack. Once the carrier and read migrations invert those readers, its only remaining
justification — escaping references — is gone, but the accessor and its externally-witnessed read
path still exist as a second access verb alongside `open`. (Its self-witnessed twin, the transitional
`read`, is retired in parallel by [value-reads-to-open](value-reads-to-open.md); this item clears
`attach`, so the two reach the single-access-verb end-state together.)

**Acceptance criteria.**

- `Sealed` exposes a single access verb, `open`: `attach` and the externally-witnessed
  witness-borrow read path are deleted here, the self-witnessed `read` having been deleted by
  [value-reads-to-open](value-reads-to-open.md), and no call site references either.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Open-only is the destination — decided.* A single access verb is the substrate's target
  surface; this item is the cleanup that confirms no consumer still needs the transitional one.
- *Gated on a clean residue — decided.* If a consumption path proved un-invertible and still holds
  an `attach`, that is surfaced here rather than silently retaining the verb; the residue is closed
  before deletion, not worked around.

## Dependencies

**Requires:**

- [Borrow-bounded `attach` fallback](externally-witnessed-attach.md) — the verb this removes;
  framestorage already landed a scope-specialized one, so this is no longer a no-op.
- [Migrate the loose witness-borrow wrappers onto `Sealed`](migrate-reattach-helpers.md) — clears
  the continuation / contract and value-path reference reattaches.
- [Migrate result-slot value reads to `open`](value-reads-to-open.md) — clears the value-read
  escapes.
- [Migrate scope-handle reads to `open`](scope-reads-to-open.md) — clears the scope-read escapes.

**Unblocks:** none.
