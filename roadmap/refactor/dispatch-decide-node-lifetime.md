# Narrow the dispatch decide surface from `'run` to `'node`

The dispatch decide layer is monomorphic in `'run`, but the data it captures is pinned by
the slot's per-call cart, not the run. Retype the surface to a cart-scale `'node` lifetime so
the continuation's stored-then-restored captures name the lifetime that actually witnesses
them.

**Problem.** A slot's continuation is stored erased (`ErasedCont`) on the lifetime-free node
and restored each step with an unsafe `'run` reattach
([`run_step`](../../src/machine/execute/run_loop.rs): `let cont: NodeCont<'run> = unsafe {
erased_cont.reattach() }`). The `'run` is not independently justified at that boundary — it
is propagated from the whole dispatch decide surface, every piece of which is typed at `'run`:
[`Outcome<'run, 'run>`](../../src/machine/execute/outcome.rs),
[`decide(expr: KExpression<'run>)`](../../src/machine/execute/dispatch.rs),
[`ResumeFn<'run>` / `DepFinish<'run>` / `CatchFinish<'run>`](../../src/machine/execute/outcome.rs),
and the `working_expr: KExpression<'run>` the closures capture
([`dispatch/exec.rs`](../../src/machine/execute/dispatch/exec.rs)).

That `'run` on `working_expr` is load-bearing in exactly one place:
[`ExpressionPart::Future(Carried<'run>)`](../../src/machine/model/ast.rs) — resolved
sub-results spliced back into the working AST. Every other part owns its data (`String`,
`KLiteral`), which is why [`parse<'a>(input: &str)`](../../src/parse/expression_tree.rs)
leaves `'a` unconstrained: a fresh AST is lifetime-free. At the splice site
([`dispatch/ctx.rs`](../../src/machine/execute/dispatch/ctx.rs): `working_expr.parts[*slot].value
= ExpressionPart::Future(*value)`) the spliced value was just **pull-lifted into this node's
frame** and re-exposed to `'run` only by an unsafe up-reattach
([`deps_for_builtin`](../../src/machine/execute/outcome.rs)). The referent lives in the slot's
cart arena, or a strict ancestor that the cart's `outer` chain pins — i.e. it is cart-scale
(`'node`), not run-scale. A bare-name `Future` resolving to a run-arena binding is still
covered, since cart ⊆ run. TCO cannot invalidate it: tail reuse draws from the *reserve*,
never the active cart ([`acquire_tail_frame`](../../src/machine/execute/runtime.rs),
[`ambient.rs`](../../src/machine/execute/ambient.rs)), so the cart carrying the futures
outlives its own step. So the cart `Rc` the step already holds live is the true witness for
every cont capture, and `'run` is a strictly wider over-approximation of `'node`.

**Acceptance criteria.**

- The dispatch decide surface is parameterized by a cart-scale `'node` lifetime in place of
  `'run`: `Outcome`, `decide`, `ResumeFn`, `DepFinish`, `CatchFinish`, `NodeCont`, and the
  `working_expr` they thread carry `'node`, bounded by the slot's held cart `Rc`.
- The continuation reattach in `run_step` targets `'node` (the lifetime the held cart `Rc`
  witnesses), not a fabricated `'run`.
- `deps_for_builtin` no longer reattaches the pull-lifted dep terminals *up* to `'run` — the
  values are delivered to the finish at the `'node` scale they already live at.
- The only remaining `'run` value-lifetime fabrication on the execution path is
  [`pin_carried_to_run`](../../src/machine/execute/outcome.rs) at the genuine run-root drain
  ([`run_program`](../../src/machine/execute/runtime/interpret.rs)), where a consumer-less
  top-level terminal is re-homed into the run arena.
- Behavior is unchanged and the Miri audit slate stays green: no `Future` referent outlives
  the slot's cart, verified by the slate's erase/reattach coverage.

**Directions.**

- *Scope: the decide side only, not the value-store side — decided.* The terminal-value path
  is already node-scale (`finalize_terminal` is `'o -> 'o`, consumer-pull lifts into the
  consumer arena); this item retypes the AST/continuation path that still rides `'run`.
- *`'node` bound — open.* `'node` is the lifetime of the slot's cart `Rc`. Whether the
  reattach becomes a safe borrow of the held cart's arena or stays an `unsafe` reattach to a
  narrower-but-still-fabricated lifetime depends on whether `'node` can be spelled as a borrow
  the borrow-checker accepts at the `run_step` call site. Recommended: spike the safe-borrow
  form first (prefer a compile-enforced borrow over a relabelled unsafe); fall back to the
  narrowed reattach only if the slot's own-then-rotate cart handling defeats a borrow.
- *TCO-carry audit — open.* Confirm every re-dispatch that carries already-spliced `Future`
  parts into a later step keeps the same cart (`FramePlacement::Inherit`) or re-lifts, so no
  spliced referent is stranded by a cart rotation. This audit gates the retype; if a path
  strands a referent, that path re-lifts before the retype lands. Recommended: enumerate the
  `finish_eager_subs` → `Continue` exits ([`dispatch/ctx.rs`](../../src/machine/execute/dispatch/ctx.rs),
  [`dispatch/keyworded.rs`](../../src/machine/execute/dispatch/keyworded.rs)) and classify each
  frame placement.

## Dependencies

Builds on the shipped node-lifetime lift + finalize-within-step substrate (see
[design/memory-model.md](../../design/memory-model.md) and
[design/per-call-arena-protocol.md](../../design/per-call-arena-protocol.md)); that work
already node-scaled the value path, leaving the decide/AST path as the remaining `'run`
over-approximation this item removes.

**Requires:** none — builds on shipped substrate.

**Unblocks:** none tracked yet.
