# Unify the type-name resolution path

Collapse the duplication on the `TypeIdentifier` → `KType` resolution path: the
three structurally-identical `Done`/`Park`/`Unbound` outcome enums, and the
builtin-table fallback repeated across the path's resolver layers.

**Problem.** Two distinct duplications ride the same type-name resolution path.

*The outcome enums.* Type-name resolution threads its result through three
separate three-arm enums, each `success | Park(Vec<NodeId>) | Unbound(String)`:

- [`ElabResult<'a>`](../../src/machine/model/types/resolver.rs) — `Done(KType<'a>)`
  (an **owned** type), the model-layer elaboration result.
- [`TypeIdentifierResolution<'a>`](../../src/machine/execute/dispatch/resolve_type_identifier.rs)
  — `Done(&'a KType<'a>)` (an **arena reference**), the scope-bound memoized result.
- [`TypeLeafCarrier<'a>`](../../src/machine/execute/dispatch/resolve_type_identifier.rs)
  — `Resolved(&'a KType<'a>)`, the type-channel carrier for the three bare-leaf call
  sites.

The `Park` and `Unbound` arms are byte-identical across all three. The only real
distinction is the success payload — owned `KType<'a>` (model produces it) versus
arena `&'a KType<'a>` (execute memoizes it). `TypeIdentifierResolution` and
`TypeLeafCarrier` carry the *same* arena reference and differ only in the success
variant's name (`Done` versus `Resolved`); each layer re-wraps the previous with a
hand-written three-arm `match` (`resolve_type_identifier.rs:50-63`, `93-99`). The
`TypeLeafCarrier` adapter additionally re-clones and re-allocates an
already-arena-allocated reference (`scope.region.alloc_ktype(kt.clone())`,
`resolve_type_identifier.rs:95`), a redundant allocation on every resolved leaf.

*The resolver layers.* Resolving a `TypeIdentifier` to a `KType` is spread across
three functions that all bottom out at the same `KType::from_name` builtin-table
lookup:

- [`KType::from_type_identifier`](../../src/machine/model/types/ktype_resolution.rs)
  — a scopeless, thin wrapper over `from_name`, with one non-test caller
  ([`ExpressionPart::resolve_for`](../../src/machine/model/ast.rs), which falls
  back to the `KType::Unresolved` transient on a miss).
- [`elaborate_type_identifier`](../../src/machine/model/types/resolver.rs) — the
  scope-aware model-layer resolver; after its recursive-set and placeholder checks
  it *also* bottoms out at `from_name`, in both the `Value` and `UnboundName` arms.
- [`Scope::resolve_type_identifier`](../../src/machine/execute/dispatch/resolve_type_identifier.rs)
  — the execute-layer memoized wrapper over `elaborate`, adding the cache and a
  finalize gate over the model result.

The builtin-table fallback `from_type_identifier` performs is thus repeated inside
`elaborate_type_identifier`; `from_type_identifier` earns its place only as a
scopeless shortcut for the one bind-time caller that runs before a scope exists.

**Acceptance criteria.**

- A single generic `ResolveOutcome<T> { Done(T), Park(Vec<NodeId>), Unbound(String) }`
  is the only resolution-outcome shape; `ElabResult<'a>` and
  `TypeIdentifierResolution<'a>` are type aliases over it
  (`ResolveOutcome<KType<'a>>` and `ResolveOutcome<&'a KType<'a>>`), and
  `TypeLeafCarrier` no longer exists as a distinct type.
- The owned-to-arena lift is expressed by one `map_done` combinator on
  `ResolveOutcome`, not by hand-written per-layer `match` re-wraps; the `Park` and
  `Unbound` arms forward unchanged.
- No resolved-leaf path clones-and-re-allocates an already-arena-allocated `&KType`;
  the carrier holds the memo's reference directly.
- The three former `TypeLeafCarrier` match sites
  ([`attr.rs`](../../src/builtins/attr.rs),
  [`dispatch.rs`](../../src/machine/execute/dispatch.rs),
  [`single_poll.rs`](../../src/machine/execute/dispatch/single_poll.rs)) and the
  `ElabResult` consumers match on `ResolveOutcome` variants with unchanged behavior.
- The bare-leaf builtin-table fallback (`KType::from_name`) is reached through one
  owner on the resolution path: `from_type_identifier` either folds into the
  `elaborate`/`resolve` stack or routes its `from_name` call through the shared
  owner `elaborate` also uses, so the fallback is no longer written twice.
- `ExpressionPart::resolve_for`'s bind-time bare-leaf lowering reaches that same
  owner and still surfaces `KType::Unresolved` on a miss.

**Directions.**

- *One generic `ResolveOutcome<T>`, three instantiations — decided.* The success
  payload varies (`KType<'a>` owned vs `&'a KType<'a>`); `Park`/`Unbound` are shared.
  `TypeLeafCarrier` collapses fully into the `&'a KType<'a>` instantiation (its
  `Resolved` arm was a renamed `Done`).
- *No lifetime parameter on the bare enum — decided.* `'a` rides entirely inside `T`,
  so the enum is `ResolveOutcome<T>`, not `ResolveOutcome<'a, T>`; the `Park`/`Unbound`
  arms stay lifetime-free.
- *Owned→ref lift via `map_done` — decided.* `Scope::resolve_type_identifier` lifts the
  elaborator's owned `Done(KType)` to an arena `Done(&KType)` through
  `ResolveOutcome::map_done`, replacing the hand-written re-wrap and consuming the memo
  reference without a re-clone.
- *Home module for the generic — open.* `ElabResult` lives in the model layer
  (`model/types/resolver.rs`) and `TypeIdentifierResolution` in execute
  (`execute/dispatch/resolve_type_identifier.rs`); the shared generic must live in the model
  layer so execute can alias it. Recommended: define `ResolveOutcome<T>` beside
  `ElabResult` in `resolver.rs` and re-export from the types module.
- *Fold `from_type_identifier`, or keep it as the scopeless entry — open.* Either
  (a) delete `from_type_identifier` and lower `ExpressionPart::resolve_for` through a
  shared scopeless `from_name` fallback that `elaborate` also calls, or (b) keep
  `from_type_identifier` as the named scopeless entry and route `elaborate`'s builtin
  fallback through it so the `from_name` call lives in one place. Recommended: (b) —
  `resolve_for` runs at bind time before a scope exists, so a named scopeless entry
  stays earning its keep; the duplication to remove is the *repeated fallback*, not the
  entry point.

## Dependencies

A standalone refactor-hygiene item on the type-resolution path
([design/typing/elaboration.md](../../design/typing/elaboration.md)); update that doc
if the resolution-outcome vocabulary or resolver-layer structure it names changes. The
resolver-layer fold touches `model/types/resolver.rs`, `model/types/ktype_resolution.rs`,
and `execute/dispatch/resolve_type_identifier.rs`.

**Requires:** none — engine-internal.

**Unblocks:** none tracked yet.
