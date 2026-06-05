# Collapse the bare-leaf type resolvers

Fold the synchronous `coerce_type_token_value` into the scheduler-aware
`Scope::resolve_type_expr` bridge so a bare type-name leaf has one resolution path.

**Problem.** A bare type-name leaf (`Number`, `Point`, `OrderedSig`) resolves through
two functions that overlap on the scope-chain walk:

- `elaborate_type_expr`
  ([`src/machine/model/types/resolver.rs`](../../src/machine/model/types/resolver.rs)),
  wrapped by the memoized `Scope::resolve_type_expr` bridge
  (`src/machine/execute/dispatch/resolve_type_expr.rs:36`), returns a `KType`, can
  `Park` on an unfinalized type binder, and threads recursion (`RecursiveRef`) plus
  synchronous SCC cycle-close. Used by FN/FUNCTOR signatures, LET RHS, ascription, and
  return-type slots.
- `coerce_type_token_value` (`src/machine/execute/dispatch/resolve_type_expr.rs:71`)
  returns a `KObject` carrier *synchronously* — it cannot park — and is the resolver
  for the `BareTypeLeaf` dispatch arm (`src/machine/execute/dispatch/single_poll.rs:235`),
  the `body_type_lhs` ATTR entry (`src/builtins/attr.rs:103`), and one
  `src/machine/execute/dispatch.rs:97` callsite.

Both bottom out in the same `resolve_type` lookup plus `KType::from_name` fallback. The
duplicated scope-walk-and-park spine is the value-dispatch name-resolution protocol
(`resolve_dispatch.rs`'s `NameOutcome`) expressed a second time. `coerce`'s one distinct
behavior — recovering a paired value-side carrier for a `UserType`/`Module` identity
instead of synthesizing a `KTypeValue` — is documented as typically-dead: no nominal
binder dual-writes anymore (SIG was the last), so the recovery misses and falls through
to synthesis (`src/machine/execute/dispatch/resolve_type_expr.rs:63-66`). And because the
dispatch arm returns `NodeStep::Done` unconditionally, a bare `:(T)` naming a
not-yet-finalized binder has no parking path the compound type forms get.

**Impact.**

- *One bare-leaf resolver.* The `BareTypeLeaf` arm and `body_type_lhs` route through the
  memoized `resolve_type_expr` bridge with a thin `KTypeValue`-wrapping adapter, so
  type-leaf resolution has a single implementation behind a single cache.
- *Bare type leaves gain parkability.* Routing through the bridge lets a `:(T)` naming a
  pending binder park on its producer like every compound type form, closing the
  synchronous-only gap.
- *Dead paired-recovery branch removed.* The `lookup_with_chain` recovery and its
  defensive fall-through retire once the recovery is confirmed unreachable.

**Directions.**

- *Fold target — decided.* Collapse into the existing `Scope::resolve_type_expr` bridge
  (already memoized and park-capable), not the raw `elaborate_type_expr`. Each of the
  three `coerce` callsites wraps the bridge's `&KType` in `KObject::KTypeValue`.
- *Paired-carrier recovery — open.* Verify the recovery branch is dead — that no
  dual-writing binder survives — then delete it; otherwise keep it behind the adapter.
  Recommended: prove dead and remove.
- *Visibility chain — deferred.* `coerce` threads `chain: Option<&LexicalFrame>` via
  `resolve_type_with_chain`; `elaborate_type_expr` uses plain `resolve_type`. The fold
  must not simply drop the chain (that bakes in the carve-out). The prerequisite
  [Retire the lexical-visibility carve-outs](position-dependent-type-resolution.md)
  chain-gates the elaborator, after which both sides agree and the two chain-passing
  callsites (`single_poll.rs:241`, `dispatch.rs:97`) fold without a semantics choice.
- *Parkability — open.* Parking needs the `Combine`-over-producers `NodeStep::Replace`
  shape the sigil lane already uses (`NodeStep` has no `Park` variant). Decide whether to
  gain parking now — one behavior change to validate — or keep the arm synchronous and
  dedup only the lookup. Recommended: gain parking, since the synchronous-only arm is the
  latent gap, not a feature.

## Dependencies

**Requires:**

- [Type language via dispatch](../../design/typing/type-language-via-dispatch.md) — the
  `SigiledTypeExpr` → sub-Dispatch substrate that already routes every compound type form
  through dispatch, leaving the bare leaf as the residual non-dispatch path.
- [Retire the lexical-visibility carve-outs](position-dependent-type-resolution.md) —
  settles the visibility-chain direction below: it chain-gates the elaborator, so the
  fold target inherits one visibility rule instead of dropping the cutoff.

**Unblocks:** none tracked yet.

A leaf cleanup whose only roadmap-level prerequisite is the visibility redesign; it
blocks no open item, and is independent of the in-flight type-language surface work.
