# Consumer mints ride the delivery envelope

**Problem.** Every consumer-side reach mint decomposes the delivery envelope
by hand: `Scope::host_reach_of` / `adopted_reach_of`
([scope.rs](../../src/machine/core/scope.rs) — both funnel into the private
`Scope::reach_of`, which calls the library's `Carrier::mint_into` in
[carrier.rs](../../workgraph/src/witnessed/carrier.rs)) take a
`(witness, host)` pair, so each adoption site reads
`carrier.witness(), Some(carrier.host())`
([let_binding.rs](../../src/builtins/let_binding.rs),
[val_decl.rs](../../src/builtins/val_decl.rs),
[newtype_def.rs](../../src/builtins/newtype_def.rs),
[using_scope.rs](../../src/builtins/using_scope.rs),
[branch_walk.rs](../../src/builtins/branch_walk.rs)) — a borrowed bare frame
pin at a consumer site, against the boundary rule that pins travel as
`Delivered` envelopes or owned `FrameSet` witnesses
([design/scheduler-library.md](../../design/scheduler-library.md)). The
bundled verb `Delivered::mint_reach`
([delivered.rs](../../workgraph/src/witnessed/delivered.rs)) — whose body is
exactly this mint — ships with zero callers. The same split shows up as
koan-side `unsafe`: `Scope::adopt_sealed`
([scope.rs](../../src/machine/core/scope.rs)) mints the reach and then
re-anchors the sealed value in a second, separately-unsafe step
(`Erased::reattach`), with the soundness link between the two carried by a
comment rather than a signature.

**Acceptance criteria.**

- Koan's consumer-side reach mints take the delivery envelope and route
  `Delivered::mint_reach`; no koan call site passes `witness()` and `host()`
  separately into a mint.
- `Delivered::host()` does not appear at any koan mint site.
- `Carrier::mint_into` is off koan's mint path — crate-internal to workgraph,
  or narrowed to the explicit resident entry chosen below.
- The resident mint case (a value already living in an ambiently covered
  region, today the `None`-host arm of `Scope::reach_of`) has its own explicit
  entry rather than riding `Some`/`None` on a shared parameter.
- Copy-free adoption is one library verb: the mint and the re-anchor it
  justifies are fused behind a `Delivered` adopt entry, and
  [scope.rs](../../src/machine/core/scope.rs) contains no `unsafe`.

**Directions.**

- *Consumer mint verb — decided.* `Delivered::mint_reach` is the sole
  consumer-side mint verb; the decomposed `(witness, host)` form is retired,
  not blessed. (Contract-audit fork ruling, 2026-07-07.)
- *Resident entry — open.* (a) Keep a narrow public `Carrier::mint_into` for
  the resident path; (b) add an envelope-free `Witnessed`-level mint twin so
  `mint_into` goes `pub(crate)`. Recommended: (b) — the surface then states
  "resident" in its type rather than in a `None` argument.
- *Adopt verb — decided.* `Scope::adopt_sealed`'s mint-then-reattach pair
  fuses into one library entry on the envelope (`Delivered::adopt_into`:
  the `Residence::Kept` mint, then the re-anchor that mint justifies), so
  the caller cannot split them and the reattach `unsafe` moves into the
  library beside `retype`. (Unsafe-elimination review, 2026-07-07.)
- *Adopt omission — open.* The adopt verb's omit predicate: (a)
  caller-supplied, as `mint_reach` takes today — trust surface identical to
  the existing mint path, no retention change; (b) fixed to the destination
  region only — fully structural soundness, at the cost of materializing
  ancestor-region pins into child arenas (over-retention). Recommended: (a).

## Dependencies

**Requires:** none.

**Unblocks:**

- [Publishing the workgraph crate](workgraph-extraction.md) — mint-surface
  visibility settles before the API freezes.
