# Eager-parts fallback masks unresolvable dispatch with a misleading diagnostic

A keyword-headed call that provably cannot resolve — because an already-evaluated
argument slot rejects — still defers to eager sub-dispatch of an unrelated literal
operand, surfacing that operand's incidental error instead of a clean "no matching
overload".

**Problem.** The post-walk dispatch fallback
([resolve_dispatch.rs](../src/machine/execute/dispatch/resolve_dispatch.rs)) ranks
its outcomes *placeholders > eager parts > Unbound > pending overload > Unmatched*
(see [scheduler.md § Post-walk dispatch fallback precedence](../design/typing/scheduler.md#post-walk-dispatch-fallback-precedence)).
Rank 2 fires whenever the expression carries *any* eager-shaped part
(`expr_has_eager_part`), deferring to sub-dispatch them and re-resolving. But when
the reason no overload admits is an **already-evaluated** slot — a value no amount
of eager evaluation can change — eager sub-dispatch of an unrelated part is
provably futile: it cannot rescue admission, and instead leaks that part's own
error in place of the honest `Unmatched`.

`(x y) FROM 5` exhibits it (the `FROM` projection,
[record_projection.rs](../src/builtins/record_projection.rs)). The `record` slot is
typed `:{}` and rejects the Number `5`, so the lone `FROM` overload never admits.
The fields operand `(x y)` was *already admitted* by its `KExpression` slot, yet
the fallback treats it as an eager part, sub-dispatches it, and evaluates `x` as a
bare name — surfacing `unbound name 'x'` rather than "no record-shaped `FROM`
overload". The keyword set is fixed no matter what `(x y)` evaluates to, and
evaluating a `KExpression`-destined part can only *break* its slot's admission, so
the deferral can never resolve the call — it only degrades the diagnostic. The
`from_non_record_operand_is_dispatch_non_match` test in
[record_projection.rs](../src/builtins/record_projection.rs) pins this current,
wrong behavior.

**Impact.**

- *An unresolvable call surfaces a clean "no matching overload" diagnostic at the
  call site* rather than an unrelated sub-part's incidental error.
- *A literal-capture (`KExpression`) operand is dispatched at most once* — admitted
  by its slot and left alone, even when a sibling slot rejects.
- *The fallback skips provably-futile eager sub-dispatch*, saving the scheduler the
  work of evaluating parts that cannot change any candidate overload's admission.

**Directions.**

- *Refine the rank-2 gate — open.* Replace `expr_has_eager_part(expr)` with a
  predicate that fires only when eager evaluation *could* flip a rejecting slot:
  some candidate overload (same bucket key) has a rejecting slot whose part is a
  still-unevaluated eager expression. When every candidate's rejection is a hard,
  already-evaluated slot, skip rank 2 and fall through to `Unbound` / `Unmatched`.
  This preserves the load-bearing `(maybe) some 42` case the precedence exists for
  (there the eager part *is* the rejecting head, so the gate still fires). It needs
  the fallback to carry per-candidate / per-slot "rejecting ∧ eager" correlation
  that the current per-name `bare_outcomes` cache doesn't track — the open work is
  plumbing that signal.
- *Walk-time short-circuit vs fallback-only — open.* Whether to fix this purely in
  the post-walk fallback, or also surface `Unmatched` earlier when a bucket's
  keyword matches but its value slots hard-reject during the walk.

## Dependencies

**Requires:** none — a self-contained dispatch-diagnostics fix over the shipped
post-walk fallback.

**Unblocks:** none tracked.
