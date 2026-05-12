# Testing and linting Koan

Three layers, each with a distinct job:

1. **`cargo test`** — every unit test in the crate, run on every push and PR.
2. **`cargo clippy` / `cargo fmt`** — lints and formatting.
3. **The Miri audit slate** — targeted memory-safety coverage for every unsafe
   site in the runtime, run under tree borrows.

## Unit tests

```sh
cargo test                  # all unit tests
cargo test parse::          # one module
cargo test -- --nocapture   # show stdout
```

Each module keeps its tests in a `#[cfg(test)] mod tests` block alongside the
code (parser, scheduler, dispatch, interpreter all have suites). After smoke-
testing a feature or bug fix, capture the smoke test as a unit test in the
nearest module's `tests` block.

CI runs `cargo build --verbose && cargo test --verbose` on push and PR against
`master` (see [.github/workflows/rust.yml](.github/workflows/rust.yml)).

## Linting and formatting

```sh
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Run these locally before pushing. Clippy is configured per-crate in
[Cargo.toml](Cargo.toml); per-site `#[allow(...)]` is fine when the lint is
wrong (e.g., the `clippy::unnecessary_cast` allows in
[src/runtime/machine/core/arena.rs](src/runtime/machine/core/arena.rs) and
[src/runtime/model/values/module.rs](src/runtime/model/values/module.rs) where the
through-`'static` cast is required by the lifetime-erasure pattern).

## Miri audit slate

The audit slate is the load-bearing memory-safety check. It runs every unsafe
site in the runtime — lifetime-erasure transmutes, raw-pointer round-trips,
interior mutation under live shared borrows, the cycle gate that prevents
self-referential `Rc<CallArena>` storage — under Miri's tree-borrows mode, with
zero process-exit leaks and zero UB required for sign-off.

The model the slate signs off on is documented in
[design/memory-model.md](design/memory-model.md#verification). Future re-runs
against post-stage-1 unsafe sites are tracked in
[roadmap/module-system-2-scheduler.md](roadmap/module-system-2-scheduler.md).

### Command of record

```sh
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test --quiet -- <test-names>
```

The first run under a fresh Miri target dir takes several minutes to compile;
subsequent runs are 1–3 min per test. Triage workflow (per-test re-runs,
pinned-id allocation tracking) lives in
[.claude/skills/miri/SKILL.md](.claude/skills/miri/SKILL.md).

### Recent full-slate run durations

The five most-recent full-slate runs, newest first. The Miri skill appends a
new entry on every full-slate run and trims to five so this list stays bounded.
Use the most-recent entry as the baseline expectation when scheduling a run.

<!-- slate-durations:start -->
- 2026-05-10: 281.61s — 24 tests, 0 leaks, 0 UB
- 2026-05-09: 266.18s — 23 tests, 0 leaks, 0 UB
- 2026-05-09: 218.77s — 21 tests, 0 leaks, 0 UB
<!-- slate-durations:end -->

### The slate

22 tests, grouped by the unsafe site each pins down. Names below are the exact
test identifiers; pass them after `--` in the command above.

**Singleton transmutes** ([src/runtime/machine/core/arena.rs](src/runtime/machine/core/arena.rs)) — the `'static`→`'a`
re-annotation on the `NULL_HOLDER` / `TRUE_HOLDER` / `FALSE_HOLDER` shared singletons.

- `singleton_ref_independent_of_arena_lifetime`
- `singletons_aliasable`

**`CallArena` lifetime erasure** ([src/runtime/machine/core/arena.rs](src/runtime/machine/core/arena.rs)) — the
`*const Scope<'static>` round-trip plus the `Rc<CallArena>` chain that keeps
per-call arenas pinned across re-borrow.

- `call_arena_scope_survives_subsequent_alloc`
- `call_arena_scope_survives_subsequent_alloc_via_raw_ptr_roundtrip`
- `call_arena_scope_repeated_calls_alias`
- `call_arena_chained_outer_frame_walkable`
- `call_arena_scope_re_anchored_into_struct_alongside_rc`

**`RuntimeArena` interior mutation under live borrows** ([src/runtime/machine/core/arena.rs](src/runtime/machine/core/arena.rs)).

- `runtime_arena_alloc_while_prior_ref_live`

**Cycle gate** ([src/runtime/machine/core/arena.rs](src/runtime/machine/core/arena.rs)) — `alloc_object` redirects
a value carrying a self-anchored `Rc<CallArena>` to the escape arena, breaking
the storage cycle that closure-escape returns can otherwise produce.

- `alloc_object_redirects_self_anchored_value_to_escape_arena`

**Closure escape and TCO replace** ([src/runtime/builtins/call_by_name.rs](src/runtime/builtins/call_by_name.rs)) — a closure
returned from its defining frame remains invocable, including with parameters
whose values are arena-allocated in the dying frame.

- `closure_escapes_outer_call_and_remains_invocable`
- `escaped_closure_with_param_returns_body_value`

**`Scope::add` re-entry** ([src/runtime/machine/core/scope.rs](src/runtime/machine/core/scope.rs)) — adding a binding while
a `data` borrow is live queues onto a pending list and drains on borrow drop,
so the conditional-defer path doesn't violate the `RefCell` invariant.

- `add_during_active_data_borrow_queues_and_drains`

**MATCH on `Tagged` recursion** ([src/runtime/builtins/match_case.rs](src/runtime/builtins/match_case.rs)) — the
`outer_frame` chain keeps the call-site arena alive across TCO replace when a
user-fn recurses through a `Tagged` parameter via MATCH.

- `recursive_tagged_match_no_uaf`

**KFuture anchor decision** ([src/runtime/machine/execute/lift.rs](src/runtime/machine/execute/lift.rs)) — the targeted anchor: a KFuture
whose descendants don't borrow into the dying arena lifts with `frame: None`;
one with a `Future(&KObject)` allocated in the dying arena anchors with
`frame: Some(rc)`.

- `unanchored_kfuture_no_arena_borrow_does_not_anchor`
- `unanchored_kfuture_with_arena_borrow_does_anchor`

**Per-call arena reclamation** ([src/runtime/builtins/fn_def.rs](src/runtime/builtins/fn_def.rs)) — repeated user-fn
calls grow the run-root arena by one lifted return value per call, with all
per-call scaffolding freed at call return.

- `repeated_user_fn_calls_do_not_grow_run_root_per_call`

**Module / Signature lifetime erasure** ([src/runtime/model/values/module.rs](src/runtime/model/values/module.rs)) — `Module`
and `Signature` carry their captured scope as `*const Scope<'static>` and
re-attach `'a` via transmute on access; `Module::type_members` mutates a
`RefCell<HashMap>` while a `&'a Module<'a>` is live (the opaque-ascription
shape).

- `module_child_scope_transmute_does_not_dangle`
- `signature_decl_scope_transmute_does_not_dangle`
- `module_type_members_refcell_mutation_with_held_module_ref`

**MODULE body Combine continuation** ([src/runtime/builtins/module_def.rs](src/runtime/builtins/module_def.rs)) — the
MODULE body schedules a `Combine` whose `finish` closure captures the child
scope and runs on the outer scheduler's main loop after every body statement
terminalizes. The captured-reference and finalize-write shapes are the
post-refactor analogue of the `module_child_scope_transmute_does_not_dangle`
site, exercised through the actual scheduler path.

- `module_body_dispatch_does_not_dangle`

**Dispatch-time placeholder parking** ([src/runtime/machine/execute/run.rs](src/runtime/machine/execute/run.rs)) — the bare-Identifier
short-circuit and the replay-park (per
[design/execution-model.md § Dispatch-time name placeholders](design/execution-model.md#dispatch-time-name-placeholders))
both rewrite a parked slot's work and walk the producer's terminal `&KObject`
out of `results[from]` after the notify-walk wakes the consumer. The reference
must remain valid across the wake; these tests are minimal-shape mirrors.

- `lift_park_minimal_program_for_miri`
- `replay_park_minimal_program_for_miri`

### Adding tests to the slate

Add a test to the slate when a new unsafe site lands — a transmute, raw-pointer
round-trip, interior-mutation pattern under a live shared borrow, or a cycle
shape that storage-side reasoning can't rule out. Tests are minimal-shape
mirrors of the unsafe operation, not end-to-end feature tests; they fail when
Miri reports UB or a leak, not on values.

When you add or remove a slate test, update the list above (the section
structure mirrors the unsafe-site groupings, so a new test lands under the
group it pins down — or under a new group if it's a new shape) and re-run the
slate to confirm the line count matches.
