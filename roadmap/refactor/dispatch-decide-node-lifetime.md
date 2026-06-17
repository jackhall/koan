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

The decide surface also stores each slot's scope as
[`NodeScope::Anchored(ScopePtr<'static>)`](../../src/machine/execute/nodes.rs), reattached at the
read boundary ([`reattach_node_scope`](../../src/machine/execute/dispatch/ctx.rs)) under a contract
that the pointee lives for all of `'run` — the one decide-surface scope handle that asserts run
scale. The scopes it covers are the block scopes the `InScope` builtins allocate (USING, MODULE,
SIG, TRY) and the top-level run root. But each `InScope` child is allocated in
[`ctx.scope.arena`](../../src/builtins/using_scope.rs) — the call-site arena, a cart ancestor the
active cart's `outer_frame` chain pins — and the root is adopted by the `run_frame` cart
([`CallArena::adopting`](../../src/machine/core/arena.rs)). So those scopes are cart-pinned too;
`Anchored` over-approximates them to `'run` exactly as the value path did.

**Acceptance criteria.**

- `NodeScope::Anchored` is removed; every slot scope handle is cart-witnessed — `Yoked`
  re-projecting the cart's own scope, or a `YokedChild(ScopePtr)` re-projecting a cart-reachable
  child — reattached bounded by the held frame `Rc`, never at a free `'run`. `NodeScope` names no
  run-lived pointer.
- The dispatch decide surface is parameterized by a single cart-scale `'node` lifetime in place of
  the `Outcome<'run, 's>` split: `Outcome`, `decide`, `ResumeFn`, `DepFinish`, `CatchFinish`,
  `NodeCont`, and the `working_expr` they thread carry `'node`, bounded by the slot's held cart `Rc`.
- The continuation reattach in `run_step` targets `'node` (the lifetime the held cart `Rc`
  witnesses), not a fabricated `'run`. It remains an `unsafe` erase→reattach — the continuation is
  stored erased across a park, so no borrow spans its storage — but is witnessed by the held cart.
- `deps_for_builtin` and `shorten_outcome` are deleted: once `Outcome` and the dep values share the
  one decide lifetime, the up-reattach (`'s`→`'run`) and down-reattach (`'run`→`'s`) bridges are
  unnecessary.
- The only remaining `'run` value-lifetime fabrication on the execution path is
  [`pin_carried_to_run`](../../src/machine/execute/outcome.rs) at the genuine run-root drain
  ([`run_program`](../../src/machine/execute/runtime/interpret.rs)), where a consumer-less
  top-level terminal is re-homed into the run arena.
- Behavior is unchanged and the Miri audit slate stays green: no `Future` referent or scope handle
  outlives the slot's cart, verified by the slate's erase/reattach coverage.

**Directions.**

- *Scope: the decide side only, not the value-store side — decided.* The terminal-value path
  is already node-scale (`finalize_terminal` is `'o -> 'o`, consumer-pull lifts into the
  consumer arena); this item retypes the AST/continuation path that still rides `'run`.
- *Delete `Anchored` first; it is the prerequisite — decided.* The retype bottoms out at this one
  genuinely-run-looking handle, so it lands first. A cart-reachable child rides a
  `YokedChild(ScopePtr)` reattached bounded by the frame `Rc`, classified by walking the active
  cart's scope `outer` chain for the child's arena; the top-level root rides the `run_frame` cart
  (which adopts it). Both keep each scope where it already lives — no fresh cart — so transparent
  `InScope` bind forwarding (`using_scope.rs`: block binds outlive the block) is preserved.
- *`InScope` body homing — decided: re-project, do not re-cart.* The alternative — give each
  `InScope` body a fresh `FreshChild` cart so its scope equals the cart scope and yokes trivially —
  is rejected: a fresh body cart drops the block's forwarded binds at body end, breaking USING /
  MODULE semantics. The `YokedChild` re-projection keeps the single-cart model.
- *Continuation reattach: stays `unsafe`, cart-witnessed — decided.* A compile-enforced borrow is
  not reachable: the continuation is stored erased across a park, so no borrow spans store→restore.
  The reattach narrows its *target* from `'run` to the cart-witnessed decide lifetime but remains an
  erase→reattach. (This resolves the earlier prefer-a-borrow question against a borrow.)

## Dependencies

Builds on the shipped node-lifetime lift + finalize-within-step substrate (see
[design/memory-model.md](../../design/memory-model.md) and
[design/per-call-arena-protocol.md](../../design/per-call-arena-protocol.md)); that work
already node-scaled the value path, leaving the decide/AST path as the remaining `'run`
over-approximation this item removes.

**Requires:** none — builds on shipped substrate.

**Unblocks:** none tracked yet.
