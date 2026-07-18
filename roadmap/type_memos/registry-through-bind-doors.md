# Run registry through bind doors and builtin seeding

Thread the run's `TypeRegistry` through the overload-dedupe path and reorder builtin
seeding after run-frame establishment, so every registry in the tree is the run frame's.
Part of the arc landing
[design/typing/type-registry.md](../../design/typing/type-registry.md).

**Problem.** `PartialEq for DeferredReturn`
([signature.rs](../../src/machine/model/types/signature.rs)) compares captured
return expressions via `expression_equal`, which needs a `&TypeRegistry`, but a trait
impl takes no parameter — so it constructs a cold registry per comparison: the one
registry in the tree no run frame owns, against the home-and-reach rule in
[design/typing/type-registry.md](../../design/typing/type-registry.md). Answers stay
exact (a verdict is a pure cache over content digests; a miss falls through to the
structural walk that is the source of truth), so the cost is the rule violation, not
wrong answers. The comparison is reachable only from the overload dedupe in
`Bindings::try_apply` ([bindings.rs](../../src/machine/core/bindings.rs)), and every
path into it — the `Scope` bind doors
([scope/registry.rs](../../src/machine/core/scope/registry.rs)), the pending drain
(run-loop-driven), ascription replay — runs inside the run with the ambient registry
reachable, with one exception: builtin seeding. `default_scope`
([builtins.rs](../../src/builtins.rs)) builds the run-root scope and registers every
builtin before any runtime exists, so no run registry can be passed there — even
though no builtin registers a `Deferred` return (`ReturnType::Deferred` is minted
only by `fn_def` at runtime), so the seeding path never consults the registry it
cannot be handed.

**Acceptance criteria.**

- `ExpressionSignature::exact_equal` and the return-type comparison behind it take a
  `&TypeRegistry` parameter; the `PartialEq`/`Eq` impls for `ReturnType` and
  `DeferredReturn` no longer exist.
- No call site constructs a `TypeRegistry` outside the run frame's mint
  (`CallFrame::adopting`); the registry parameter is threaded from the run at every
  dedupe-reaching site — the `Scope` bind doors, `Bindings::try_apply`/`replay`, and
  the pending drain.
- Builtin seeding runs after the run frame is established: production sequences bare
  root scope → runtime → run frame → seed, and each builtin's `register` receives the
  run frame's registry.
- The full test slate is green.

**Directions.**

- *Structural comparison, not canonical-render strings — decided.* The
  `Expression`-arm identity compare stays the structural `expression_equal` walk;
  lowering it to a `summarize()` string compare (as `DeferredReturnSurface` uses for
  its hash shadow) is rejected — string-lowered comparison is the wart structural
  value equality exists to avoid.
- *Explicit method over trait impl — decided.* The registry cannot ride a `PartialEq`
  impl's signature, so overload-identity comparison becomes a named method taking
  `&TypeRegistry`, and the trait impls are deleted rather than left delegating.
- *Seeding reorder is safe — decided.* `ensure_run_frame` wraps the already-built
  run-root scope via `CallFrame::adopting` with no builtin lookups, so no circularity
  blocks establishing the run frame before seeding.
- *Test-harness seed registry — open.* Tests mint a fresh runtime (and registry) per
  phase by design; the harness must hold a run frame before seeding. Options: a
  harness-internal seed runtime constructed and dropped around scope construction; a
  harness that returns the scope paired with its first runtime; or a
  production-shaped single-runtime harness rework.

## Dependencies

**Requires:** none — builds on the shipped run-frame registry.

**Unblocks:**

- [Interned type content behind Copy handles](interned-type-content.md) — once all
  type content lives in the run frame's registry graph, builtin seeding's own type
  construction needs that registry in hand, so the reorder and threading must land
  first.
