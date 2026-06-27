# `alloc_ktype` returns `Witnessed`

Migrate the type allocation family onto `yoke`, so every `KType` born in a per-call region comes back
already bundled with the set of regions it reaches — taking the last value off the single-frame
reconstruction and deleting it.

**Problem.** [`region.alloc_ktype`](../../src/machine/core/arena.rs) (~38 call sites — the
highest-volume family) returns a bare `&'a KType` that is not witnessed at all; like the object path, a
transitional `Witnessed::new` would assert co-location in prose rather than guarantee it by
construction, even though the `yoke` / `merge` constructors and the production witness plumbing now
exist. The one region-referencing variant — a `KType::Module` naming its child scope — has its reach
recovered not from a carrier but by the single-frame read-out
[`reached_frame`](../../src/machine/execute/lift.rs) (the `KType::Module` arm). It is the **last** user
of that reconstruction: its child scope lives in a region distinct from the module's producer frame, so
— unlike a `KObject::KFunction`, whose defining frame *is* its producer frame and whose reach the
dep-result currency already carries — the uniform retain at the
[`relocate`](../../src/machine/execute/run_loop.rs) cannot drop to the carried set until
`KType::Module`'s construction names the child-scope reach on its carrier.

**Acceptance criteria.**

- `alloc_ktype` returns a `KType` bundled with the set of regions it reaches, built inside the witness
  closure — most `KType`s are owned / `Rc`-shared and `yoke` directly, while a region-referencing
  variant (a `KType::Module` naming its child scope) witnesses that child scope's sealed
  [reach-set](scope-reach-set.md) — so a region-resident type is born co-located by construction.
- The type family carries no `Witnessed::new`: most `KType`s `yoke` directly, while the one
  region-referencing variant (`KType::Module`) folds its child-scope reach — co-location enforced by
  the brand, never asserted.
- A lifted `KType::Module`'s reached region is read off its carrier's witness set. With its last user
  converted, the read-out [`reached_frame`](../../src/machine/execute/lift.rs) reconstruction **and**
  the per-frame [`FrameStorage.retained`](../../src/machine/core/arena.rs) field are both **deleted** —
  binding reach now lives on the per-scope [reach-set](scope-reach-set.md), and no value remains on the
  bare-copy relocation path, so the over-approximated step `pin` becomes exact.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Its own PR, after the object family — decided.* At ~38 sites the `ktype` conversion is its own PR;
  it reuses the shipped dep-result plumbing and lands *after* the object family's
  [embedding-site inversions](alloc-object-embedding-sites.md).
- *Construction inversion, not post-hoc bundling — decided.* The type is built inside the witness
  closure; a `for<'b>` closure cannot accept an already-built `KType<'a>`. Most variants `yoke` (owned
  / `Rc` data); a `KType::Module` folds its child-scope reach.
- *The scope witness rides the type, not `alloc_scope` — decided.* A `KType::Module`'s child scope is
  alloc'd via `alloc_scope`, but the witness that keeps its region alive rides the `KType::Module`
  carrier (the value), not the scope handle, so `alloc_scope` itself stays bare `&'a`. The operand is
  the child scope's sealed [reach-set](scope-reach-set.md) plus its home frame added on lift.
- *Completes the single-frame reconstruction's deletion — decided.* The object path's last dependence
  on `reached_frame` is retired by
  [`alloc-object-embedding-sites`](alloc-object-embedding-sites.md); this item takes the final
  `KType::Module` user off it. With binding reach on the per-scope reach-set and every relocated value
  carrying its reach on its dep currency, the uniform retain at the `relocate` loses its last caller, so
  `reached_frame` and the per-frame `retained` field are deleted here.

## Dependencies

**Requires:**

- [`alloc_object` embedding sites return `Witnessed`](alloc-object-embedding-sites.md) — retires the
  object path's dependence on `reached_frame`, the retirement this item completes by taking the final
  `KType::Module` user off it.

**Unblocks:**

- [Migrate the consumption reads onto `open`](reads-to-open.md) — the transitional `read` can retire
  only once both construction channels are off the bare read-out, of which this is the type half.
