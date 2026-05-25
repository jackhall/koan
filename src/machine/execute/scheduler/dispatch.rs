use crate::machine::core::source::Spanned;
use crate::machine::model::Parseable;
use crate::machine::{
    Frame, KError, KErrorKind, NodeId, ResolveOutcome, Resolution, Resolved, Scope,
};
use crate::machine::model::ast::{ExpressionPart, KExpression};

use super::super::nodes::{LiftState, NodeOutput, NodeStep, NodeWork};
use super::Scheduler;

impl<'a> Scheduler<'a> {
    /// Dispatch driver: a linear pipeline through five phases.
    ///
    /// 1. **`try_short_circuit`** — bare-name match in the current scope. A `Value` hit
    ///    terminates immediately; a `Placeholder` hit installs a park edge and rewrites the
    ///    slot to a `Lift`. `Unbound` and non-bare-name shapes fall through.
    /// 2. **`Scope::resolve_dispatch`** — one chain walk yielding a [`Resolved`],
    ///    `Ambiguous(n)`, `Deferred`, or `Unmatched`. `Ambiguous` and `Unmatched` surface
    ///    as structured errors. `Deferred` jumps to phase 5; `Resolved` continues.
    ///    A keyword-headed call to a not-yet-registered function fails here: function
    ///    dispatch goes through the `functions` bucket, which does not consult the
    ///    `placeholders` table, so it has no forward-reference park (unlike value/type
    ///    slots, which route through `Scope::resolve` in phases 1 and 4).
    /// 3. **Placeholder install** — if the picked function carried a `pre_run` extractor,
    ///    install its dispatch-time name placeholder against this slot's `NodeId`.
    /// 4. **`apply_auto_wrap` + `try_replay_park`** — rewrite the expression's
    ///    `wrap_indices` parts into sub-Dispatches; check `ref_name_indices` for
    ///    already-errored producers, parking on the rest.
    /// 5. **`schedule_deps`** — schedule the resolution's `eager_indices` plus any other
    ///    `Expression` / `ListLiteral` / `DictLiteral` parts as sub-nodes, building a
    ///    `Bind` slot. If no subs needed, bind the function directly and step to its
    ///    body.
    ///
    /// See [design/execution-model.md § Dispatch-time name placeholders](../../../../design/execution-model.md#dispatch-time-name-placeholders)
    /// for the bare-name short-circuit, placeholder install, auto-wrap pass, and
    /// replay-park rules referenced above.
    pub(super) fn run_dispatch(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // Phase 1.
        if let Some(step) = self.try_short_circuit(&expr, scope, idx) {
            return Ok(step);
        }

        // Phase 2. `Ambiguous` / `Unmatched` propagate as `Err` (rather than
        // `NodeStep::Done(NodeOutput::Err(_))`) so they surface at `Scheduler::execute`'s
        // return value, matching today's `scope.dispatch(...)?` shape.
        let resolved = match scope.resolve_dispatch(&expr) {
            ResolveOutcome::Resolved(r) => r,
            ResolveOutcome::Ambiguous(n) => {
                return Err(KError::new(KErrorKind::AmbiguousDispatch {
                    expr: expr.summarize(),
                    candidates: n,
                }));
            }
            ResolveOutcome::Unmatched => {
                return Err(KError::new(KErrorKind::DispatchFailed {
                    expr: expr.summarize(),
                    reason: "no matching function".to_string(),
                }));
            }
            ResolveOutcome::Deferred => {
                // No overload picks against the bare shape, but the expression carries
                // eager parts whose evaluation may surface matching types. Schedule them
                // through the standard eager fallthrough and rebind on completion.
                return Ok(self.schedule_eager_fallthrough(expr, scope, idx));
            }
            ResolveOutcome::ParkOnProducers(producers) => {
                // A tentative tie hinged on a forward-referenced bare name. Park on its
                // producer(s) and re-dispatch on wake, when the strict-pass peek can read
                // the bound type.
                return Ok(self.park_pending_and_redispatch(producers, expr, idx));
            }
            ResolveOutcome::UnboundName(name) => {
                return Err(KError::new(KErrorKind::UnboundName(name)));
            }
        };

        // Phase 2.5: install dispatch-time placeholder for the binder slot, if any.
        if let Some(name) = resolved.placeholder_name.as_ref() {
            if let Err(e) = scope.install_placeholder(name.clone(), NodeId(idx)) {
                return Ok(NodeStep::Done(NodeOutput::Err(e)));
            }
        }

        // Phase 3: pure-transform auto-wrap.
        let rewritten = apply_auto_wrap(expr, &resolved.slots.wrap_indices);

        // Phase 4: replay-park check.
        match self.try_replay_park(&rewritten, &resolved, scope, idx) {
            ReplayParkResult::Done(step) => return Ok(step),
            ReplayParkResult::Continue => {}
        }

        // Phase 5: schedule eager subs from the resolution's indices.
        Ok(self.schedule_deps(rewritten, &resolved, scope, idx))
    }

    /// Park `idx` on each still-pending producer and rebuild it as a re-Dispatch of `expr`.
    /// Shares the replay-park guards: a producer that already terminalized with an error
    /// propagates (parking on a dead slot would deadlock); one that would close a cycle is
    /// skipped; if no parkable producer remains, the call is a genuine no-match. On wake the
    /// re-Dispatch re-runs resolution, where the now-bound name lets the strict-pass peek
    /// pick. Drives the tentative-tie [`ResolveOutcome::ParkOnProducers`] path from both
    /// `run_dispatch` and `run_bind`.
    pub(super) fn park_pending_and_redispatch(
        &mut self,
        producers: Vec<NodeId>,
        expr: KExpression<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        let mut to_wait: Vec<NodeId> = Vec::new();
        for p in producers {
            if self.is_result_ready(p) {
                // Terminal while its placeholder is still set ⇒ the producer errored
                // (success clears the placeholder); propagate rather than park on a dead slot.
                if let Err(e) = self.read_result(p) {
                    let frame = Frame::from_expr("<dispatch-park>", &expr);
                    return NodeStep::Done(NodeOutput::Err(
                        e.clone_for_propagation().with_frame(frame),
                    ));
                }
            } else if !self.deps.would_create_cycle(p, NodeId(idx)) {
                to_wait.push(p);
            }
        }
        if to_wait.is_empty() {
            return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::DispatchFailed {
                expr: expr.summarize(),
                reason: "no matching function".to_string(),
            })));
        }
        for p in &to_wait {
            self.deps.add_park_edge(*p, NodeId(idx));
        }
        NodeStep::Replace {
            work: NodeWork::Dispatch(expr),
            frame: None,
            function: None,
        }
    }

    /// Phase 1. Bare-name short-circuit. `Some(step)` only fires on `Value` (terminate with
    /// the bound value) or `Placeholder` (install park edge, rewrite to `Lift`). `Unbound`
    /// and non-bare-name shapes return `None` for the caller to continue.
    fn try_short_circuit(
        &mut self,
        expr: &KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Option<NodeStep<'a>> {
        if let [Spanned { value: ExpressionPart::Identifier(name), .. }] = expr.parts.as_slice() {
            match scope.resolve(name) {
                Resolution::Value(obj) => Some(NodeStep::Done(NodeOutput::Value(obj))),
                Resolution::Placeholder(producer_id) => {
                    // Notify edge, not Owned: the producer is a sibling slot this Lift
                    // only parks on for a wake — it is not part of this slot's reclaim
                    // subtree. `add_park_edge` installs the forward wake on
                    // `notify_list[producer]` and bumps `pending_deps[idx]` in the same
                    // atomic body; `free` skips past Notify edges via `owned_children`.
                    // Producer-not-terminal precondition: `Resolution::Placeholder` is
                    // only returned between submission and terminalization of the
                    // placeholder's slot, so `producer_id` is not yet terminal here.
                    self.deps.add_park_edge(producer_id, NodeId(idx));
                    Some(NodeStep::Replace {
                        work: NodeWork::Lift(LiftState::Pending(producer_id)),
                        frame: None,
                        function: None,
                    })
                }
                // Unbound falls through so `value_lookup`'s body produces the structured
                // `UnboundName` error.
                Resolution::Unbound => None,
            }
        } else {
            None
        }
    }

    /// Phase 4. Walk `resolved.slots.ref_name_indices` against `expr`: a slot whose name resolves
    /// to a still-pending placeholder needs a park edge; a slot whose producer already
    /// terminalized with an error propagates that error. Returns `Continue` when the slot
    /// can proceed to phase 5, or `Done` when a park or propagation took over.
    fn try_replay_park(
        &mut self,
        expr: &KExpression<'a>,
        resolved: &Resolved<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> ReplayParkResult<'a> {
        let mut producers_to_wait: Vec<NodeId> = Vec::new();
        for &i in &resolved.slots.ref_name_indices {
            let name = match expr.parts.get(i).map(|p| &p.value) {
                Some(ExpressionPart::Identifier(n)) => n.as_str(),
                // Bare leaf Type-tokens in literal-name slots park on the same placeholder
                // rails as Identifier — `IntOrd :| OrderedSig` waits on a forward-declared
                // `MODULE IntOrd` the same way `LET y = (x)` waits on `LET x = …`.
                // Parameterized Type parts (List<…>, etc.) are structural type-syntax, not
                // look-up targets.
                Some(ExpressionPart::Type(t))
                    if matches!(t.params, crate::machine::model::ast::TypeParams::None) =>
                {
                    t.name.as_str()
                }
                // wrap_indices and ref_name_indices are disjoint by construction.
                _ => continue,
            };
            match scope.resolve(name) {
                Resolution::Placeholder(producer_id) => {
                    if self.is_result_ready(producer_id) {
                        // A `ref_name_indices` slot whose producer has already
                        // terminalized but whose placeholder is still set means the
                        // producer errored (success would have cleared the placeholder
                        // via `bind_value`); propagate the error rather than parking on a
                        // dead slot.
                        if let Err(e) = self.read_result(producer_id) {
                            let frame = Frame::from_expr("<replay-park>", expr);
                            let propagated = e.clone_for_propagation().with_frame(frame);
                            return ReplayParkResult::Done(NodeStep::Done(NodeOutput::Err(
                                propagated,
                            )));
                        }
                    } else if self.deps.would_create_cycle(producer_id, NodeId(idx)) {
                        // Trivial cycle: `LET Ty = Ty` — the value-side `Ty` sub-Dispatch
                        // is an Owned child of the LET binder and is about to park on
                        // that same LET's placeholder. Parking would deadlock; surface a
                        // structured cycle error instead.
                        let kerr = KError::new(KErrorKind::ShapeError(format!(
                            "cycle in type alias `{name}`",
                        )));
                        return ReplayParkResult::Done(NodeStep::Done(NodeOutput::Err(kerr)));
                    } else {
                        producers_to_wait.push(producer_id);
                    }
                }
                Resolution::Value(_) | Resolution::Unbound => {}
            }
        }
        if !producers_to_wait.is_empty() {
            // Notify edges: replay-park parks on sibling producers (often top-level slots)
            // the rewritten Dispatch does not own. `free` must not transit through these
            // into the producer's subtree. Producer-not-terminal precondition:
            // `producers_to_wait` is built from `is_result_ready(p) == false` above, so
            // every `p` here is known-not-terminal at install time.
            for p in &producers_to_wait {
                self.deps.add_park_edge(*p, NodeId(idx));
            }
            return ReplayParkResult::Done(NodeStep::Replace {
                work: NodeWork::Dispatch(expr.clone()),
                frame: None,
                function: None,
            });
        }
        ReplayParkResult::Continue
    }

    /// Phase 5 — `Resolved` arm. Single loop over `expr.parts` branching on whether the
    /// picked function is a lazy candidate (`resolved.slots.eager_indices.is_some()`):
    /// - **Lazy candidate** (the picked sig has a `KType::KExpression` slot bound by an
    ///   `ExpressionPart::Expression`): only the carried `eager_indices` — `Expression`
    ///   parts in *non-*`KExpression` slots — schedule as sub-Dispatches; every other
    ///   part rides through unchanged (including lazy `Expression` parts in `KExpression`
    ///   slots, which the receiving builtin dispatches itself).
    /// - **Not a lazy candidate**: schedule every `Expression` / `ListLiteral` /
    ///   `DictLiteral` part as a sub.
    ///
    /// If no subs were scheduled, bind the picked function directly and step into its
    /// body via `invoke_to_step`.
    fn schedule_deps(
        &mut self,
        expr: KExpression<'a>,
        resolved: &Resolved<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        let mut new_parts = Vec::with_capacity(expr.parts.len());
        let mut subs: Vec<(usize, NodeId)> = Vec::new();
        match resolved.slots.eager_indices.as_deref() {
            Some(eager_indices) => {
                // Lazy arm: sub-Dispatch the eager indices AND the wrap indices. The
                // wrap indices have already been rewritten by `apply_auto_wrap` into
                // `Expression([Type(name)])` / `Expression([Identifier(name)])` shapes,
                // so they're equivalent in shape to eager Expression parts. Without
                // sub-Dispatching them, an auto-wrapped slot would arrive at `bind` as
                // a `KExpression` value and fail the slot's type check. Pre-`SIG_WITH`
                // this gap was invisible because `lazy_eager_indices` rejected any
                // function with auto-wrap-able bare-name slots; the relaxation in
                // `lazy_eager_indices` is paired with this sub-Dispatch coverage.
                let wrap_indices = &resolved.slots.wrap_indices;
                for (i, part) in expr.parts.into_iter().enumerate() {
                    if eager_indices.contains(&i) || wrap_indices.contains(&i) {
                        let inner = match part.value {
                            ExpressionPart::Expression(boxed) => *boxed,
                            // Post-`apply_auto_wrap`, every `wrap_indices` slot is an
                            // `Expression(_)`; pre-wrap-aware `eager_indices` also only
                            // flags `Expression` parts. Anything else here is a
                            // classification bug.
                            _ => unreachable!(
                                "eager_indices / wrap_indices only flag Expression parts post-auto-wrap",
                            ),
                        };
                        let sub_id = self.add(NodeWork::Dispatch(inner), scope);
                        subs.push((i, sub_id));
                        new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                    } else {
                        new_parts.push(part);
                    }
                }
            }
            None => {
                for (i, part) in expr.parts.into_iter().enumerate() {
                    let span = part.span;
                    match part.value {
                        ExpressionPart::Expression(boxed) => {
                            let sub_id = self.add(NodeWork::Dispatch(*boxed), scope);
                            subs.push((i, sub_id));
                            new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                        }
                        ExpressionPart::ListLiteral(items) => {
                            let agg_id = self.schedule_list_literal(items, scope);
                            subs.push((i, agg_id));
                            new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                        }
                        ExpressionPart::DictLiteral(pairs) => {
                            let agg_id = self.schedule_dict_literal(pairs, scope);
                            subs.push((i, agg_id));
                            new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                        }
                        other => new_parts.push(Spanned { value: other, span }),
                    }
                }
            }
        }
        let new_expr = KExpression::new(new_parts);
        if subs.is_empty() {
            // No subs: bind the picked function directly. Spliced `Future(&'a KObject)`
            // references survive `results[dep] = None` because the objects live in arenas
            // tied to lexical scope.
            match resolved.function.bind(new_expr) {
                Ok(future) => self.invoke_to_step(future, scope, idx),
                Err(e) => NodeStep::Done(NodeOutput::Err(e)),
            }
        } else {
            let bind_id = self.add(NodeWork::Bind { expr: new_expr, subs }, scope);
            self.defer_to_lift(idx, bind_id)
        }
    }

    /// Phase 5 — `Deferred` arm. No overload matched the bare shape, but the expression
    /// carries eager parts. Schedule every `Expression` / `ListLiteral` / `DictLiteral`
    /// part as a sub-Dispatch and build a `Bind` slot. After the subs resolve,
    /// `run_bind` calls `Scope::resolve_dispatch` again on the rewritten expression with
    /// `Future(_)` parts — typed slots that rejected `Expression` accept the resulting
    /// `Future(KObject)`.
    fn schedule_eager_fallthrough(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        let mut new_parts = Vec::with_capacity(expr.parts.len());
        let mut subs: Vec<(usize, NodeId)> = Vec::new();
        for (i, part) in expr.parts.into_iter().enumerate() {
            let span = part.span;
            match part.value {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add(NodeWork::Dispatch(*boxed), scope);
                    subs.push((i, sub_id));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                }
                ExpressionPart::ListLiteral(items) => {
                    let agg_id = self.schedule_list_literal(items, scope);
                    subs.push((i, agg_id));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                }
                ExpressionPart::DictLiteral(pairs) => {
                    let agg_id = self.schedule_dict_literal(pairs, scope);
                    subs.push((i, agg_id));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                }
                other => new_parts.push(Spanned { value: other, span }),
            }
        }
        let new_expr = KExpression::new(new_parts);
        // `Deferred` implies `expr_has_eager_part(&expr) == true`, so `subs` is non-empty
        // by construction.
        debug_assert!(
            !subs.is_empty(),
            "Deferred ⇒ at least one eager part; got zero subs",
        );
        let bind_id = self.add(NodeWork::Bind { expr: new_expr, subs }, scope);
        self.defer_to_lift(idx, bind_id)
    }
}

/// Forward-reference fallback for the `Unmatched` arm of `run_dispatch`. Walks the
/// expression's `Keyword` parts and consults [`Scope::resolve`] for each — a `Placeholder`
/// hit means a sibling binder's `pre_run` already announced this dispatch name, so the
/// consumer should park on the producer rather than fail with `no matching function`.
/// First hit wins (FN's `pre_run` extracts the first signature `Keyword`, which is the
/// registered dispatch name — same one a consumer's first Keyword would match).
///
/// `Resolution::Value` and `Resolution::Unbound` are non-hits: a value-side binding under
/// the same name is fine (the bucket lookup already failed against the function shape,
/// so this is a real shape mismatch, not a pending one), and an unbound name is the
/// terminal "no matching function" case.
/// Phase 3. Pure transform: rewrite each `wrap_indices` slot's bare-Identifier or bare
/// leaf Type-token into a single-name sub-Expression so it re-enters via the bare-name
/// short-circuit and routes through the Identifier or TypeExprRef overload of
/// `value_lookup`. Other variants fall through unchanged — `wrap_indices` is built from
/// is-bare-name parts, so any other variant would be a classifier bug; restore rather than
/// panic.
fn apply_auto_wrap<'a>(expr: KExpression<'a>, wrap_indices: &[usize]) -> KExpression<'a> {
    let mut parts = expr.parts;
    for &i in wrap_indices {
        let placeholder = Spanned::bare(ExpressionPart::Identifier(String::new()));
        let original = std::mem::replace(&mut parts[i], placeholder);
        let span = original.span;
        let new_value = match original.value {
            ExpressionPart::Identifier(name) => ExpressionPart::Expression(Box::new(
                KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier(name))]),
            )),
            ExpressionPart::Type(t) => ExpressionPart::Expression(Box::new(KExpression::new(
                vec![Spanned::bare(ExpressionPart::Type(t))],
            ))),
            other => other,
        };
        parts[i] = Spanned { value: new_value, span };
    }
    KExpression {
        parts,
        span: expr.span,
        file: expr.file,
    }
}

/// Replay-park branch result: `Done` means a park was installed or a producer-error was
/// propagated and the caller should short-circuit; `Continue` means no park needed and the
/// caller should proceed to phase 5.
enum ReplayParkResult<'a> {
    Done(NodeStep<'a>),
    Continue,
}
