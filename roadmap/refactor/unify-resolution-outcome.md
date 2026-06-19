# Unify the type-resolution-outcome enums

Collapse the three structurally-identical `Done`/`Park`/`Unbound` enums on the
type-name resolution path into one generic `ResolveOutcome<T>`.

**Problem.** Type-name resolution threads its result through three separate
three-arm enums, each `success | Park(Vec<NodeId>) | Unbound(String)`:

- [`ElabResult<'a>`](../../src/machine/model/types/resolver.rs) — `Done(KType<'a>)`
  (an **owned** type), the model-layer elaboration result.
- [`ResolveTypeExprOutcome<'a>`](../../src/machine/execute/dispatch/resolve_type_identifier.rs)
  — `Done(&'a KType<'a>)` (an **arena reference**), the scope-bound memoized result.
- [`TypeLeafCarrier<'a>`](../../src/machine/execute/dispatch/resolve_type_identifier.rs)
  — `Resolved(&'a KType<'a>)`, the type-channel carrier for the three bare-leaf call
  sites.

The `Park` and `Unbound` arms are byte-identical across all three. The only real
distinction is the success payload — owned `KType<'a>` (model produces it) versus
arena `&'a KType<'a>` (execute memoizes it). `ResolveTypeExprOutcome` and
`TypeLeafCarrier` carry the *same* arena reference and differ only in the success
variant's name (`Done` versus `Resolved`); each layer re-wraps the previous with a
hand-written three-arm `match` (`resolve_type_identifier.rs:50-63`, `93-99`). The
`TypeLeafCarrier` adapter additionally re-clones and re-allocates an
already-arena-allocated reference (`scope.arena.alloc_ktype(kt.clone())`,
`resolve_type_identifier.rs:95`), a redundant allocation on every resolved leaf.

**Acceptance criteria.**

- A single generic `ResolveOutcome<T> { Done(T), Park(Vec<NodeId>), Unbound(String) }`
  is the only resolution-outcome shape; `ElabResult<'a>` and
  `ResolveTypeExprOutcome<'a>` are type aliases over it
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
  (`model/types/resolver.rs`) and `ResolveTypeExprOutcome` in execute
  (`execute/dispatch/resolve_type_identifier.rs`); the shared generic must live in the model
  layer so execute can alias it. Recommended: define `ResolveOutcome<T>` beside
  `ElabResult` in `resolver.rs` and re-export from the types module.

## Dependencies

A standalone refactor-hygiene item on the type-resolution path
([design/typing/elaboration.md](../../design/typing/elaboration.md)); update that doc
if the resolution-outcome vocabulary it names changes.

**Requires:** none — engine-internal.

**Unblocks:** none tracked yet.
