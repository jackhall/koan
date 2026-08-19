# Deferred error-trace rendering

**Problem.** The success path pays for error traces that never fire. `working_frame`
([src/machine/execute/decide.rs](../../src/machine/execute/decide.rs)) builds a
`TraceFrame` eagerly: it renders the whole working expression to a `String` through
`summarize`, converts the function tag into an owned `String`, and resolves a `SourceLoc`
whose path is cloned. Every eager-subs dispatch pays one at
`install_eager_subs`'s `<bind>` frame and every park pays one at `<dispatch-park>`
([src/machine/execute/decide/keyworded.rs](../../src/machine/execute/decide/keyworded.rs));
the operator-chain lane pays its own two
([src/machine/execute/decide/operator_chain.rs](../../src/machine/execute/decide/operator_chain.rs)).
The frame is held in case a dep errors and dropped unused when none does — the common
case. A dhat profile of the audit shapes (2026-08-18) attributes ≈27 of the tail loop's
206 allocations per step to the `summarize` family (`ExpressionPart` 13, `KExpression` 8,
`WorkingExpression` 4, `WorkingPart` 2), plus the `TraceFrame`'s own tag and path
`String`s — a share on par with the whole record-keying term that
[Run-scoped label interning](string-interning.md) targets.

**Acceptance criteria.**

- A dispatch that completes without error renders no expression summary and allocates no
  trace text: the eager `summarize` and path-clone sites on the success path are gone.
- What a step retains against a possible dep error contains no rendered string; rendering
  happens at error-construction time.
- An error that does surface carries the same trace content as today: the function tag,
  the rendered expression, and a real `path:line:col` location.
- The recorded tail-loop baseline drops by the deferred share (≈27 per step), and
  [audit/README.md](../../audit/README.md) plus `tests/allocation_baseline.rs` are
  re-measured to the new figures.

**Directions.**

- *Capture currency — open.* What the success path holds in place of the rendered frame:
  a `Copy` capture (span, file id, `&'static str` function tag) versus a boxed lazy
  closure. Recommended: the `Copy` capture — it survives the step without heap traffic
  and defers the `SourceLoc` line/col resolve too.
- *Render source — open.* How error-time rendering recovers the expression text: slice
  the retained source text by span (via `crate::source::with`) versus re-summarizing the
  AST, which may sit in a region the error outlives. Recommended: the source-span slice.
- *Registration-time signature summaries — open.* `register_builtin` renders every
  builtin signature through `KFunction::summarize`
  ([src/machine/core/kfunction.rs](../../src/machine/core/kfunction.rs)) — ≈1,190 of the
  empty program's 2,874 startup allocations, the same eager-rendering pattern at
  registration scope. In or out of this item's scope.

## Dependencies

**Requires:** none.

**Unblocks:** none.
