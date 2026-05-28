# Stateful dispatch — Step 3: fast-lane variants

Implement the five non-`Keyworded` `DispatchState` variants in
`run_dispatch_stateful`: `BareTypeLeaf`, `SigiledTypeExpr`,
`BareIdentifier`, `FunctionValueCall`, and `TypeConstructorCall`.
Each terminalizes (or parks-and-resumes) entirely on the stateful
path. `Keyworded` continues to delegate to the legacy driver until
[step 4](stateful-dispatch-04-keyworded-variant.md).

**Problem.** After steps 1–2,
[`run_dispatch_stateful`](../../src/machine/execute/scheduler/dispatch.rs)
exists as a stub that classifies shape and delegates every variant
to the legacy `run_dispatch`. The five fast-lane variants
(`BareIdentifier`, `BareTypeLeaf`, `TypeConstructorCall`,
`FunctionValueCall`, `SigiledTypeExpr`) have no stateful
implementation, so the toggle-on path runs identical work to the
legacy path and proves nothing about the new architecture. The
five fast lanes are also the simplest variants — each terminalizes
in one step today, so they're the right place to validate the
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
- Five of seven `DispatchShape` variants run on the new driver
  under toggle-on; only `Keyworded` (and the still-delegating
  `Initialized → Keyworded` transition) remain on the legacy
  path, narrowing the remaining work scope to step 4.

**Directions.**

- **Implementation order — decided: increasing complexity.**
  - **(3a) `BareTypeLeaf`** — no park path, no fall-through.
    `Initialized → BareTypeLeaf → terminal` in one poll via
    [`coerce_type_token_value`](../../src/builtins/value_lookup.rs).
    Validates the `Initialized → <variant> → Done` transition
    end-to-end before any park machinery is involved.
  - **(3b) `SigiledTypeExpr`** — tail-replaces the slot with a
    fresh `Dispatch` of the wrapped expression (new
    `Initialized` state). Exercises the in-place slot rewrite
    path on the stateful driver.
  - **(3c) `BareIdentifier`** — first park path. On `Placeholder`,
    rewrites the slot to `Lift(LiftState::Pending(producer))`
    (same shim today's `fast_lane_bare_identifier` uses); the
    rewritten work is not `Dispatch` so `recent_wakes` is not
    consulted. The `Unbound` fall-through routes to legacy
    `value_lookup` body unchanged.
  - **(3d) `FunctionValueCall`** — first use of
    `install_combined_park` on the stateful driver. On
    `Placeholder` head, the variant stores its working state in
    `FnValueState` and installs a park edge; on wake, the
    variant re-enters via the toggle and re-attempts admission
    against the now-resolved head. Validates the `recent_wakes`
    drain path before `Keyworded` relies on it.
  - **(3e) `TypeConstructorCall`** — mirrors today's fast-lane
    handler (Struct/Tagged/Newtype/TypeConstructor heads route
    through their construction primitives via `Tail` rewrite;
    opaque/Module/unbound heads fall through to the keyworded
    `type_call` builtin). The fall-through retains its current
    shape: tail-replace the slot with a fresh `Initialized`
    `Dispatch` whose shape will classify as `Keyworded` —
    legacy handles `Keyworded` until step 4, so the fall-through
    works.

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

- [Stateful dispatch — Step 2: `recent_wakes` side-channel](stateful-dispatch-02-recent-wakes.md)

**Unblocks:**

- [Stateful dispatch — Step 4: `Keyworded` variant](stateful-dispatch-04-keyworded-variant.md)
