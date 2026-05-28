# Stateful dispatch — Step 3: fast-lane variants

Implement the five non-`Keyworded` `DispatchState` variants in
`run_dispatch_stateful`: `BareTypeLeaf`, `SigiledTypeExpr`,
`BareIdentifier`, `FunctionValueCall`, and `ConstructorCall`.
Each terminalizes (or parks-and-resumes) entirely on the stateful
path. `Keyworded` continues to delegate to the legacy driver until
[step 4](stateful-dispatch-04-keyworded-variant.md).

Four of the five sub-steps have landed: `BareTypeLeaf` (3a),
`BareIdentifier` (3b), `FunctionValueCall` (3c), and
`ConstructorCall` (3d) all route through per-variant handlers in
[`run_dispatch_stateful`](../../src/machine/execute/scheduler/dispatch.rs).
`SigiledTypeExpr` (3e) still delegates to the legacy
`run_dispatch` and ships alongside `Keyworded` in
[step 4](stateful-dispatch-04-keyworded-variant.md).

**Problem.** After steps 1–2,
[`run_dispatch_stateful`](../../src/machine/execute/scheduler/dispatch.rs)
existed as a stub that classified shape and delegated every variant
to the legacy `run_dispatch`. The five fast-lane variants
(`BareIdentifier`, `BareTypeLeaf`, `ConstructorCall`,
`FunctionValueCall`, `SigiledTypeExpr`) had no stateful
implementation, so the toggle-on path ran identical work to the
legacy path and proved nothing about the new architecture. The
five fast lanes are also the simplest variants — each terminalizes
in one step, so they're the right place to validate the
state-machine envelope before tackling `Keyworded`.

**Impact.**

- Toggle-on `cargo test` runs every fast-lane test through the
  stateful driver and passes. The new envelope is end-to-end
  validated against the existing test corpus before the
  state-rich `Keyworded` variant lands.
- The shared mutator surface (`install_combined_park`,
  `add_park_edge`, `recent_wakes` drain) is exercised through
  `BareIdentifier` and `FunctionValueCall`'s park paths, surfacing
  any cross-step integration gaps with steps 1 and 2.
- Four of six `DispatchShape` variants run on the new driver
  under toggle-on; only `Keyworded` and `SigiledTypeExpr` (and
  the still-delegating `Initialized → Keyworded` /
  `Initialized → SigiledTypeExpr` transitions) remain on the
  legacy path, narrowing the remaining work scope to step 4.
- The stateful driver serves NEWTYPE / TypeConstructor
  construction at parity with the legacy `type_call` builtin for
  non-keyworded heads; the legacy `type_call::body` arms for
  `Newtype` / `TypeConstructor` stay alive but are reached only
  on toggle-off and by the still-delegating `Keyworded` path on
  toggle-on.

**Directions.**

- **Implementation order — decided: increasing complexity.**
  - **(3a) `BareTypeLeaf`** — no park path, no fall-through.
    `Initialized → BareTypeLeaf → terminal` in one poll via
    [`coerce_type_token_value`](../../src/builtins/value_lookup.rs).
    Validates the `Initialized → <variant> → Done` transition
    end-to-end before any park machinery is involved.
  - **(3b) `BareIdentifier`** — first park path. On `Placeholder`,
    rewrites the slot to `Lift(LiftState::Pending(producer))`
    (same shim today's `fast_lane_bare_identifier` uses); the
    rewritten work is not `Dispatch` so `recent_wakes` is not
    consulted. The `Unbound` arm surfaces
    `KErrorKind::UnboundName(name)` directly — the legacy
    fall-through to `value_lookup::body_identifier` is dropped
    on the stateful driver (the legacy
    `(v :Identifier)` overload-bucket fall-through is the only
    behavior that shifts; one test
    (`dispatch_picks_identifier_over_any_regardless_of_registration_order`)
    is pinned to the legacy driver and a sibling
    `stateful_bare_identifier_surfaces_unbound_name_directly`
    pins the new contract).
  - **(3c) `FunctionValueCall`** — first use of
    `install_combined_park` on the stateful driver. On
    `Placeholder` head, the variant stores its working state in
    `FnValueState` and installs a park edge; on wake, the
    variant re-enters via the toggle and re-attempts admission
    against the now-resolved head. Validates the `recent_wakes`
    drain path before `Keyworded` relies on it.
  - **(3d) `ConstructorCall`** — mirrors the legacy
    `type_call::body` per-`UserTypeKind` branching:
    `StructType` / `TaggedUnionType` heads recover the value-side
    schema carrier via `coerce_type_token_value` and apply through
    `struct_value::apply` / `tagged_union::apply`; `Newtype` heads
    route through `newtype_construct` with the arena-resident
    `&'a KType` identity; `TypeConstructor` heads look the
    value-side schema carrier up through
    `scope.lookup_with_chain` and dispatch via
    `dispatch_constructor`. `Module` heads and any other identity
    surface `TypeMismatch { expected: "constructible Type" }` —
    same rejection the legacy driver produces; constructing a
    Module-as-value-via-functor lands with the functor-binder
    roadmap item, not here.
  - **(3e) `SigiledTypeExpr`** — deferred to
    [step 4](stateful-dispatch-04-keyworded-variant.md).
    Tail-replaces the slot with a fresh `Dispatch` of the
    wrapped expression; the stateful driver currently
    delegates this variant to the legacy `run_dispatch`
    alongside `Keyworded`.

- **Per-variant state structs — decided per-variant during
  implementation.** The variant-per-shape envelope from step 1
  makes per-struct decisions local to each variant. Fast-lane
  variants likely terminalize in one poll for the resolved case;
  the only cross-poll state is in `BareIdState` /
  `FnValueState`'s park paths. Decide concrete field sets at
  implementation time — the envelope makes those decisions
  reversible.

- **Routing inside `run_dispatch_stateful` — decided.** Match on
  `DispatchState`; for the five variants implemented here, run
  the stateful handler. For `Keyworded`, delegate to the legacy
  driver (call `run_dispatch` against the slot's `expr`). The
  `Initialized` variant runs `classify_dispatch_shape` once, then
  transitions to the matching per-variant state (which for
  `Keyworded` immediately delegates).

- **Tests — decided.** Toggle-on must keep `cargo test` green at
  the end of each sub-step (3a–3e). Variant-targeted tests live
  in
  [`src/machine/execute/scheduler/tests/dispatch_shapes.rs`](../../src/machine/execute/scheduler/tests/dispatch_shapes.rs);
  the broader keyworded suite (and any test that classifies as
  `Keyworded`) runs on legacy until step 4. No new tests in this
  step beyond what the existing corpus covers; new tests appear
  only if a sub-step exposes an unguarded behavior.

- **Risks — open per sub-step.** `BareIdentifier`'s `Lift`
  rewrite drops the `Dispatch` work variant — confirm that
  `pre_subs` was always empty for `BareIdentifier` (single-part
  expressions have no nested sub-expressions to pre-submit; the
  `Initialized.pre_subs` field carried through to
  `BareIdState.init.pre_subs` should always be empty on this
  path). `FunctionValueCall` shares `install_combined_park`
  with the legacy `Keyworded` path — keep one mutator surface
  so step-4 changes there don't drift.

## Dependencies

**Requires:**


**Unblocks:**

- [Stateful dispatch — Step 4: `Keyworded` variant](stateful-dispatch-04-keyworded-variant.md)
