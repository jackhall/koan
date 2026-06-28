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
  [reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into) — so a
  region-resident type is born co-located by construction.
- The type family carries no `Witnessed::new`: most `KType`s `yoke` directly, while the one
  region-referencing variant (`KType::Module`) folds its child-scope reach — co-location enforced by
  the brand, never asserted.
- A lifted `KType::Module`'s reached region is read off its carrier's witness set. With its last user
  converted, the read-out [`reached_frame`](../../src/machine/execute/lift.rs) reconstruction **and**
  the per-frame [`FrameStorage.retained`](../../src/machine/core/arena.rs) field are both **deleted** —
  binding reach now lives on the per-scope
  [reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into). With both
  channels witnessed, the transitional single-frame relocate-seam fold (the `reached_frame` fold in
  `relocate_dep_into_consumer`) is **removed entirely**: transient reach rides the output carriers, and
  only bound deposits use the scope set, so no value remains on the bare-copy relocation path and the
  over-approximated step `pin` becomes exact.
- The full Miri slate is green; `cargo test` and `cargo clippy --all-targets` clean.

**Directions.**

- *Its own PR, after the object family — decided.* At ~38 sites the `ktype` conversion is its own PR;
  it reuses the shipped dep-result plumbing and lands *after* the object family's construction
  inversions ([`constructors`](../../src/machine/execute/dispatch/constructors.rs),
  [`catch`](../../src/builtins/catch.rs), [FN-def `finalize`](../../src/builtins/fn_def/finalize.rs)),
  which have shipped.
- *Construction inversion, not post-hoc bundling — decided.* The type is built inside the witness
  closure; a `for<'b>` closure cannot accept an already-built `KType<'a>`. Most variants `yoke` (owned
  / `Rc` data); a `KType::Module` folds its child-scope reach.
- *The scope witness rides the type, not `alloc_scope` — decided.* A `KType::Module`'s child scope is
  alloc'd via `alloc_scope`, but the witness that keeps its region alive rides the `KType::Module`
  carrier (the value), not the scope handle, so `alloc_scope` itself stays bare `&'a`. The operand is
  the child scope's sealed
  [reach-set](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into) plus its home
  frame added on lift.
- *Completes the single-frame reconstruction's deletion — decided.* The object and user-fn-arg paths'
  last dependence on `reached_frame` is already retired: the value-embedding object sites
  ([`attr`](../../src/builtins/attr.rs), [`FROM`](../../src/builtins/record_projection.rs), the literal
  Resolved arm) `merge` their delivered carrier, and the [`let`](../../src/builtins/let_binding.rs) and
  user-fn object-arg ([`exec::invoke`](../../src/machine/execute/dispatch/exec.rs)) binds fold onto the
  scope reach-set, leaving [`reached_frame`](../../src/machine/execute/lift.rs) serving only
  `KType::Module`. This item takes that final user off it. With binding reach on the per-scope reach-set
  and every relocated value carrying its reach on its dep currency, the uniform retain at the `relocate`
  loses its last caller, so `reached_frame` and the per-frame `retained` field are deleted here.

## Dependencies

**Requires:** none — the object and user-fn-arg paths are already off `reached_frame` (the
carrier-delivered embeds and bind folds have shipped), leaving only `KType::Module` for this item.

**Unblocks:**

- [Migrate the consumption reads onto `open`](reads-to-open.md) — the transitional `read` can retire
  only once both construction channels are off the bare read-out, of which this is the type half.
