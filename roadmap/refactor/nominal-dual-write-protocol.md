# Concentrate the nominal dual-write protocol in `core::nominal`

Move the transactional "dual-write `(bindings.types, bindings.data)`"
protocol that every nominal binder rides — `register_nominal`,
`cycle_close_install_identity`, `try_register_nominal`, the
`pending_types` map and its SCC cycle-close driver — out of `Scope`
and `Bindings`'s public surface into a dedicated `core::nominal`
module. Pair the code consolidation with a single canonical design
page so the rule lives in one place in both source and prose.

**Problem.** The dual-write protocol is the single most-referenced
"invariant by convention" in the typing docs, and currently lives as
free-standing methods on
[`Scope`](../../src/machine/core/scope.rs) and
[`Bindings`](../../src/machine/core/bindings.rs) plus an SCC driver
in [`model/types/resolver.rs`](../../src/machine/model/types/resolver.rs):

- `Scope::register_nominal` / `Scope::cycle_close_install_identity`
  — the atomic-write shims.
- `Bindings::try_register_nominal` / `Bindings::pending_types` — the
  storage layer's coordinated mutator and pending-entry map.
- `model::types::resolver::close_type_cycle` — the SCC DFS that
  decides when to fire `cycle_close_install_identity` (a nominal-
  install concern, not a type-resolution concern).
- Six binder callsites in `builtins/` —
  [`struct_def.rs`](../../src/builtins/struct_def.rs),
  [`union.rs`](../../src/builtins/union.rs),
  [`module_def.rs`](../../src/builtins/module_def.rs),
  [`sig_def.rs`](../../src/builtins/sig_def.rs),
  [`result.rs`](../../src/builtins/result.rs),
  [`let_binding.rs`](../../src/builtins/let_binding.rs) — each
  reaches into `Scope` directly to register itself; the SCC dance
  involves `struct_def` and `union.rs` together.

The protocol is described as raw prose across five design docs
([`user-types.md`](../../design/typing/user-types.md),
[`modules.md`](../../design/typing/modules.md),
[`elaboration.md`](../../design/typing/elaboration.md),
[`error-handling.md`](../../design/error-handling.md),
[`functors.md`](../../design/typing/functors.md)) — the participant
*list* is restated as prose and never typed. `UserTypeKind`
enumerates four of the six participants (`Struct | Tagged | Newtype |
TypeConstructor`); `Module` and `Signature` ride separate `KType`
variants. A reader has to assemble the contract from those five docs
and seven source-file callsites, with no single artifact naming the
protocol.

**Impact.**

- The dual-write rule has one canonical source-level home
  (`core::nominal`) and one canonical doc home
  (`design/typing/nominal-dual-write.md`). Inbound references from
  the other typing docs become single cross-links instead of
  per-doc restatements.
- `NominalKind` enum names every participant explicitly, subsuming
  `UserTypeKind`'s four arms and the `Module` / `Signature` `KType`
  variants. Adding a new nominal carrier becomes a typed change.
- `Scope` and `Bindings`' public surfaces shrink to their core
  storage / lifetime concerns; the nominal-install façade becomes
  free functions on `nominal::` taking the façade by `&mut`.
- `close_type_cycle` lands next to the install primitive it drives,
  removing the type-resolver's nominal-install entanglement.

**Scoring.** Measured via `modgraph` with item-level moves of
`Scope::{register_nominal, cycle_close_install_identity}` and
`Bindings::{try_register_nominal, pending_types}` into
`koan::machine::core::nominal`, against the fresh post-Pass-14
baseline (machine 218.98, with ε=20 owner-credit and reference-loc
fixed):

| | machine Δ |
|---|---|
| Code consolidation alone | +1.97 |
| **Code + paired `design/typing/nominal-dual-write.md`** | **−0.48** |

The code consolidation by itself increases coupling (the 6 builtin
callsites + module/let_binding callers gain a cross-edge to the new
peer module), but the paired doc consolidation earns owner-credit on
the new `nominal.rs` that more than offsets the structural cost. The
doc write is half the candidate, not a follow-up.

**Directions.**

- **Standalone `core::nominal/` peer vs fold into a richer
  `core::bindings/` — open.** The candidates analysis worth-scoring
  both shapes; the scored variant above is the standalone peer.
  Recommended: standalone peer, since `Bindings` already does
  multiple jobs (storage façade, scope-bound memos) and growing it
  further fights the partition this work establishes.
- **`NominalKind` enum shape — open.** Either subsume `UserTypeKind`
  outright (one enum names everything) or wrap it (`NominalKind`
  has a `UserType(UserTypeKind)` arm plus `Module` / `Signature`
  arms). Recommended: subsume, since the participant list is the
  protocol's primary discriminator and indirection through
  `UserTypeKind` would hide it.
- **`design/typing/nominal-dual-write.md` is required, not optional
  — decided per Pass 10 scoring.** Without the doc consolidation
  landing alongside the code, the score regresses to +1.97 (machine)
  rather than the −0.48 win.

## Dependencies

**Requires:** none.

**Unblocks:** none.
