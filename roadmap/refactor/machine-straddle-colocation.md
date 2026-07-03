# Collapse the machine model/core straddle

`machine` layers cleanly `model ← core ← execute` except for ~9 model→core
back-edges (plus 1 model→execute).

**Problem.** Those back-edges are not stray imports: they are a strongly-connected
component split across the model/core boundary — `model::values::KObject` holds
`KFunction`/`FrameStorage` (all in `core`), while `core::scope::Scope`
imports `KObject` back from `model`. value ↔ scope ↔ closure is a cycle the module
boundary bisects, and the scorer charges its cross-boundary edges as `α·feedback`.
Relieving this by relocating one item or renaming one module measures *worse*
(E1 +186, E2 +227 on the machine subtree, baseline 2045.23). The reason is
structural, not a tooling artifact — the item-rewrite scorer is now
inbound+outbound+facade-correct: a single move leaves the SCC straddling, so the
cycle edges still cross the boundary and pay α, while the move adds a node and
fresh edges. The cost is quantized to the whole SCC; only co-locating the entire
component removes it.

**Acceptance criteria.**

- The cross-module SCC(s) in `machine` are co-located into one module, so their
  internal cycle edges become intra-module and the `α·feedback` charge on them is
  removed; the machine subtree score drops versus the 2045.23 baseline.
- The new module exposes a single public item to the rest of `machine` where
  possible, so `λ_facade` stays minimal.
- No behavioral change: value/closure/scope runtime semantics are untouched — a
  pure relocation, gated by `cargo test` and the Miri slate.

**Directions.**

- *Group membership — open.* Leading hypothesis: `KObject`, `KFunction`, `Scope`,
  `ScopePtr`/`ScopeId`, `FrameStorage` — the live-value-plus-environment
  cluster. Recommended: confirm membership with `modgraph propose` (until it lands,
  the hypothesis is scorable today with `rewrite item --move`) before committing to
  a cut.
- *Co-locate first, facade fallback — decided.* Prefer collapsing the SCC into one
  module. If a member genuinely cannot move (it belongs to its current layer for an
  independent reason), thin to a single facade item in place instead: the `cross`
  edges then route through one entry and pay `λ` once, though the `α` cycle cost
  remains. See [design/README.md § Foundation vs seam](../../design/README.md#foundation-vs-seam).

## Dependencies

Distinct from the
[naming-and-responsibility-audit](naming-and-responsibility-audit.md) (stale names
and duplicated responsibilities, not coupling cycles) — coordinate if both touch
the same files.

**Requires:** none.

**Unblocks:** none.
