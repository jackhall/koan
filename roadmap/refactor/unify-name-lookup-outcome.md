# Unify the value-name lookup outcomes

`Resolution` and `NameOutcome` are the value-side name-lookup disposition under two
names across the core/execute boundary; name the shared bound/parked/unbound axis
once.

**Problem.** A value-side name lookup reports its outcome through two enums whose
core arms coincide:

- [`Resolution`](../../src/machine/core/bindings.rs)
  `{ Value(&KObject), Placeholder(NodeId), UnboundName }` — the result of a
  single-scope value binding lookup, in the core layer.
- [`NameOutcome`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
  `{ Resolved(Carried), Parked(NodeId), ProducerErrored(KError), Unbound(String), Cycle(String) }`
  — the cached outcome of resolving a bare-name dispatch part, in the execute layer.

`Resolution::Value/Placeholder/UnboundName` map onto
`NameOutcome::Resolved/Parked/Unbound`: the same bound / park-on-producer / unbound
disposition under two names across the core↔execute boundary. `NameOutcome` is the
richer of the two — it carries a `Carried` (a value *or* a type leaf) where
`Resolution` carries a bare `&KObject`, and adds two execute-only states
(`ProducerErrored`, `Cycle`).

This is a distinct axis from the *type*-name resolution path that
[Unify the type-name resolution path](unify-resolution-outcome.md) owns, and from the
overload-resolution
[`ResolveOutcome`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
`{ Resolved, Ambiguous, Deferred, ParkOnProducers, UnboundName, Unmatched }` that
sits beside `NameOutcome`. Three different "resolution outcome" vocabularies share
one neighborhood, and the `ResolveOutcome` name is already taken twice over (overload
resolution here, plus the `ResolveOutcome<T>` the type-name-path item introduces).

**Acceptance criteria.**

- The bound / parked / unbound disposition shared by a single-scope value lookup and
  a bare-name dispatch resolution is named once — one enum both layers use, or one
  defined as a refinement of the other — so the `Value`↔`Resolved`,
  `Placeholder`↔`Parked`, `UnboundName`↔`Unbound` correspondence is structural, not
  parallel.
- The execute-only `ProducerErrored` and `Cycle` states stay expressible without the
  core single-scope lookup naming them or `Carried`.
- The unified name collides with neither the overload-resolution `ResolveOutcome` nor
  the type-name-path `ResolveOutcome<T>`.

**Directions.**

- *Merge vs one-derives-from-the-other — open.* The payloads differ (`&KObject` vs
  `Carried`) and `NameOutcome` has two extra arms, so no byte-identical merge is
  available. Either lift `Resolution` to carry `Carried` and the execute states (one
  enum), or define the shared three-way disposition in the core layer and have
  `NameOutcome` embed it plus its two extra arms. Recommended: the latter — the shared
  type lives in `core/bindings.rs` (execute can depend on core, not the reverse, as on
  the type-name path), so the core lookup never names `Carried` or the execute states.
- *Avoid the `ResolveOutcome` name — open.* Whatever the shared disposition is called,
  it must not reuse `ResolveOutcome` (overload resolution) or `ResolveOutcome<T>`
  (type-name path). Recommended: a distinct name such as `NameLookup`.

## Dependencies

The value-language sibling of the type-name resolution-path item (linked above); they
are independent axes but share the crowded "resolution outcome" vocabulary, so
coordinate naming so a third `ResolveOutcome` is not minted. Update
[design/typing/lookup-protocol.md](../../design/typing/lookup-protocol.md) if the
name-lookup vocabulary it names changes.

**Requires:** none — engine-internal.

**Unblocks:** none tracked yet.
