use std::collections::HashMap;
use std::rc::Rc;

use crate::dispatch::runtime::{Frame, KError, Resolution};
use crate::dispatch::kfunction::{BodyResult, NodeId};
use crate::dispatch::values::KKey;
use crate::dispatch::values::KObject;
use crate::dispatch::types::{Parseable, Serializable};
use crate::dispatch::runtime::{KFuture, Scope};
use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::nodes::{AggregateElement, NodeOutput, NodeStep, NodeWork};
use super::scheduler::Scheduler;

/// Walk `scope` and its outer chain, looking in `functions[expr.untyped_key()]` for any
/// function whose `pre_run` extractor returns `Some(name)` for `expr`. The first such
/// `(name, scope)` wins; install `placeholders[name] = NodeId(idx)` in that scope.
///
/// Idempotent on a §8 replay-park re-dispatch: if `placeholders[name]` already maps to the
/// same slot index, `install_placeholder` is a no-op. Errors with `Rebind` if either `data`
/// or `placeholders` already holds `name` and the existing placeholder doesn't match `idx`.
fn install_dispatch_placeholder<'a>(
    expr: &KExpression<'a>,
    scope: &'a Scope<'a>,
    idx: usize,
) -> Result<(), KError> {
    use crate::dispatch::kfunction::NodeId;
    let key = expr.untyped_key();
    // Walk the chain to find any candidate function with a pre_run extractor that matches
    // this expression. The *placeholder install* always lands in the dispatching scope
    // (`scope`), not the scope the function was discovered in — placeholders track
    // dispatch-time intent, which is local to the binder's call site.
    let mut current: Option<&Scope<'a>> = Some(scope);
    while let Some(s) = current {
        let candidate = {
            let functions = s.functions.borrow();
            let mut found: Option<String> = None;
            if let Some(bucket) = functions.get(&key) {
                for f in bucket.iter() {
                    if let Some(extractor) = f.pre_run {
                        if let Some(name) = extractor(expr) {
                            found = Some(name);
                            break;
                        }
                    }
                }
            }
            found
        };
        if let Some(name) = candidate {
            return scope.install_placeholder(name, NodeId(idx));
        }
        current = s.outer;
    }
    Ok(())
}

impl<'a> Scheduler<'a> {
    /// Walk an unresolved expression. If `lazy_candidate` matches, only schedule the
    /// eager-position `Expression` parts; the lazy positions ride through as `KExpression`
    /// data into a builtin slot typed `KExpression` (`FN`, `MATCH`, `UNION`). Otherwise
    /// schedule every `Expression` (and `ListLiteral`) part as a sub-dispatch / aggregate dep.
    /// Returns a `NodeStep`: `Done(Value)` for an inline-dispatched body that produced a
    /// value, `Replace { work: Lift { from: bind_id }, .. }` when it spawned a `Bind` to wait
    /// on subs (the dispatch slot is rewritten to a Lift shim that copies the bind's
    /// terminal into the slot once notify wakes it), or `Replace { work: Dispatch(expr), .. }`
    /// when the body was a tail call (the slot gets rewritten in place by the execute loop).
    pub(super) fn run_dispatch(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // §4: dispatch-time placeholder install. If the picked function (if any) supplies a
        // `pre_run` extractor, install `name → NodeId(idx)` in the dispatching scope's
        // `placeholders` so a sibling lookup that beats this slot's body still parks on it.
        // Errors (`Rebind`) surface as a `Done(Err)` rather than a panic — the program is
        // ill-formed, but the run continues to drain other slots.
        if let Err(e) = install_dispatch_placeholder(&expr, scope, idx) {
            return Ok(NodeStep::Done(NodeOutput::Err(e)));
        }

        // §1: single-Identifier short-circuit. `(some_var)` — a bare-Identifier dispatch slot.
        // Try to resolve the name directly: if it's a `Value`, return it without scheduling
        // value_lookup; if it's a `Placeholder`, park on the producer via Lift; if `Unbound`,
        // fall through and let `value_lookup` surface the structured `UnboundName` error.
        if let [ExpressionPart::Identifier(name)] = expr.parts.as_slice() {
            match scope.resolve(name) {
                Resolution::Value(obj) => {
                    return Ok(NodeStep::Done(NodeOutput::Value(obj)));
                }
                Resolution::Placeholder(producer_id) => {
                    self.node_dependencies[idx].push(producer_id.index());
                    return Ok(NodeStep::Replace {
                        work: NodeWork::Lift { from: producer_id },
                        frame: None,
                        function: None,
                    });
                }
                Resolution::Unbound => {
                    // Fall through to existing dispatch path; value_lookup's body emits
                    // `KErrorKind::UnboundName(name)` for genuinely unbound names.
                }
            }
        }

        // §7: shape-pick auto-wrap. Pick the unique most-specific candidate; if any
        // bare-Identifier parts sit in *value*-typed slots, rewrite them as single-
        // Identifier sub-Expressions so they re-enter `run_dispatch` and route through
        // §1's short-circuit. Multi-name forward references (`PLUS a b` where both `a`
        // and `b` are placeholders) compose as N independent sub-Dispatches.
        //
        // §8: replay-park. After the §7 rewrite, walk the picked function's
        // `ref_name_indices` (literal-name slots in a non-pre_run function — call_by_name's
        // verb, ATTR's identifier-lhs, type_call's verb). If any name resolves to a
        // placeholder, park the *outer* slot on the producer(s); on wake, re-dispatch the
        // same expression. By then the binder has called `bind_value` and the lookup
        // succeeds normally.
        let expr = match scope.shape_pick(&expr) {
            Some(pick) => {
                // §7 wrap: replace each bare-Identifier in a wrap_index with a sub-Expression.
                let mut parts = expr.parts;
                for i in pick.wrap_indices {
                    if let ExpressionPart::Identifier(name) =
                        std::mem::replace(&mut parts[i], ExpressionPart::Identifier(String::new()))
                    {
                        parts[i] = ExpressionPart::Expression(Box::new(KExpression {
                            parts: vec![ExpressionPart::Identifier(name)],
                        }));
                    }
                }
                let rewritten = KExpression { parts };

                // §8 replay-park check, post-§7 wrap. A match in `ref_name_indices` whose
                // producer terminalized but whose placeholder is still set indicates the
                // producer errored (otherwise `bind_value` would have removed the
                // placeholder). Propagate that error directly via `read_result` rather
                // than parking on a dead slot.
                let mut producers_to_wait: Vec<NodeId> = Vec::new();
                for i in pick.ref_name_indices {
                    let name = match rewritten.parts.get(i) {
                        Some(ExpressionPart::Identifier(n)) => n.as_str(),
                        // Slot's been wrapped by §7 (shouldn't happen — wrap and ref_name
                        // are disjoint by construction), or some other shape — skip.
                        _ => continue,
                    };
                    match scope.resolve(name) {
                        Resolution::Placeholder(producer_id) => {
                            if self.is_result_ready(producer_id) {
                                // Producer finalized but placeholder still set → producer
                                // errored. Surface that error here so the consumer doesn't
                                // park on a slot that will never wake.
                                if let Err(e) = self.read_result(producer_id) {
                                    let frame = Frame {
                                        function: "<replay-park>".to_string(),
                                        expression: rewritten.summarize(),
                                    };
                                    let propagated = e.clone_for_propagation().with_frame(frame);
                                    return Ok(NodeStep::Done(NodeOutput::Err(propagated)));
                                }
                            } else {
                                producers_to_wait.push(producer_id);
                            }
                        }
                        Resolution::Value(_) | Resolution::Unbound => {
                            // Bound value: dispatch will resolve normally. Genuinely unbound:
                            // dispatch will surface UnboundName from the body. Either way no
                            // park.
                        }
                    }
                }

                if !producers_to_wait.is_empty() {
                    for p in &producers_to_wait {
                        self.node_dependencies[idx].push(p.index());
                    }
                    return Ok(NodeStep::Replace {
                        work: NodeWork::Dispatch(rewritten),
                        frame: None,
                        function: None,
                    });
                }

                rewritten
            }
            None => expr,
        };

        if let Some(eager_indices) = scope.lazy_candidate(&expr) {
            let mut parts = expr.parts;
            let mut subs = Vec::with_capacity(eager_indices.len());
            for i in eager_indices {
                let inner = match std::mem::replace(
                    &mut parts[i],
                    ExpressionPart::Identifier(String::new()),
                ) {
                    ExpressionPart::Expression(boxed) => *boxed,
                    _ => unreachable!("lazy_candidate only flags Expression parts"),
                };
                let sub_id = self.add(NodeWork::Dispatch(inner), scope);
                subs.push((i, sub_id));
            }
            let parent = KExpression { parts };
            if subs.is_empty() {
                let future = scope.dispatch(parent)?;
                return Ok(self.invoke_to_step(future, scope));
            }
            let bind_id = self.add(NodeWork::Bind { expr: parent, subs }, scope);
            return Ok(self.defer_to_lift(idx, bind_id));
        }

        let mut new_parts = Vec::with_capacity(expr.parts.len());
        let mut subs: Vec<(usize, NodeId)> = Vec::new();
        for (i, part) in expr.parts.into_iter().enumerate() {
            match part {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add(NodeWork::Dispatch(*boxed), scope);
                    subs.push((i, sub_id));
                    // Placeholder — overwritten with `Future(result)` at Bind time.
                    new_parts.push(ExpressionPart::Identifier(String::new()));
                }
                ExpressionPart::ListLiteral(items) => {
                    let agg_id = self.schedule_list_literal(items, scope);
                    subs.push((i, agg_id));
                    new_parts.push(ExpressionPart::Identifier(String::new()));
                }
                ExpressionPart::DictLiteral(pairs) => {
                    let agg_id = self.schedule_dict_literal(pairs, scope);
                    subs.push((i, agg_id));
                    new_parts.push(ExpressionPart::Identifier(String::new()));
                }
                other => new_parts.push(other),
            }
        }
        let new_expr = KExpression { parts: new_parts };
        if subs.is_empty() {
            let future = scope.dispatch(new_expr)?;
            return Ok(self.invoke_to_step(future, scope));
        }
        let bind_id = self.add(NodeWork::Bind { expr: new_expr, subs }, scope);
        Ok(self.defer_to_lift(idx, bind_id))
    }

    /// Wire the dispatch slot at `idx` to wait on `bind_id`'s terminal: rewrite the slot's
    /// work to `Lift { from: bind_id }`, and record `bind_id` as an owned child so the
    /// chain free at slot drop walks the full sub-tree. The Replace carries no frame /
    /// function override — the slot's existing per-call frame (if any) and function label
    /// stay attached for the Done arm to use when the Lift writes its terminal.
    fn defer_to_lift(&mut self, idx: usize, bind_id: NodeId) -> NodeStep<'a> {
        self.node_dependencies[idx].push(bind_id.index());
        NodeStep::Replace {
            work: NodeWork::Lift { from: bind_id },
            frame: None,
            function: None,
        }
    }

    pub(super) fn run_bind(
        &mut self,
        mut expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // Short-circuit if any sub errored: propagate that error rather than dispatch the
        // parent. Append a frame naming the parent's expression so the trace reconstructs
        // the call chain. The sub slots stay in `node_dependencies[idx]` for the chain-free
        // at finalize to reclaim — eager free is the success-path optimization.
        for (_, dep_id) in &subs {
            if let Err(e) = self.read_result(*dep_id) {
                let frame = Frame {
                    function: "<bind>".to_string(),
                    expression: expr.summarize(),
                };
                let propagated = e.clone_for_propagation().with_frame(frame);
                return Ok(NodeStep::Done(NodeOutput::Err(propagated)));
            }
        }
        let dep_indices: Vec<usize> = subs.iter().map(|(_, d)| d.index()).collect();
        for (part_idx, dep_id) in subs {
            let value = self.read(dep_id);
            expr.parts[part_idx] = ExpressionPart::Future(value);
        }
        // Reclaim sub-Dispatch slots: their results are now spliced into `expr.parts` as
        // `Future(&'a KObject)`. The underlying objects live in arenas (lexical-scope
        // invariant), so the splice references survive `results[dep] = None`. Done before
        // `scope.dispatch` so any fresh `add()` inside the dispatched body's invoke can
        // recycle the indices immediately.
        self.reclaim_deps(idx, dep_indices);
        let future = scope.dispatch(expr)?;
        Ok(self.invoke_to_step(future, scope))
    }

    /// Eager-free path used by `run_bind`/`run_aggregate*` once each Dep's value has been
    /// consumed (spliced into the parent's expression as `Future(&KObject)` or deep-cloned
    /// into the parent's container). Clears the slot's owned-children sidecar and walks
    /// each `dep` through `free()` so the Dep slots return to the free-list. On error
    /// paths the deps are *not* eagerly freed — the chain free at slot drop reclaims them.
    fn reclaim_deps(&mut self, idx: usize, dep_indices: Vec<usize>) {
        self.node_dependencies[idx].clear();
        for d in dep_indices {
            self.free(d);
        }
    }

    pub(super) fn run_aggregate(
        &mut self,
        elements: Vec<AggregateElement<'a>>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeOutput<'a> {
        // One-line wrapper over the shared aggregate runner. Frame label is "<list>" and the
        // container builder wraps the resolved values in `KObject::List`. Dict's runner
        // can't share this wrapper (it needs paired key/value resolution + KKey conversion),
        // but the dep-resolution + frame-on-error pattern is factored into `resolve_or_err`.
        self.run_aggregate_with(idx, scope, "<list>", "list literal", elements, |items| {
            KObject::List(Rc::new(items))
        })
    }

    /// Shared aggregate-runner core. Iterates `elements`, resolving each via
    /// [`Self::resolve_or_err`] (which returns the framed `NodeOutput::Err` directly on
    /// failure), then hands the resolved `Vec<KObject>` to `build` to produce the final
    /// container value. Reclaims dep slots on success; on error leaves them for the
    /// chain-free at finalize. Used by `run_aggregate` (list literal); the dict runner
    /// stays paired-iteration for KKey conversion but reuses `resolve_or_err`.
    fn run_aggregate_with<F>(
        &mut self,
        idx: usize,
        scope: &'a Scope<'a>,
        frame_function: &str,
        frame_expression: &str,
        elements: Vec<AggregateElement<'a>>,
        build: F,
    ) -> NodeOutput<'a>
    where
        F: FnOnce(Vec<KObject<'a>>) -> KObject<'a>,
    {
        let make_frame = || Frame {
            function: frame_function.to_string(),
            expression: frame_expression.to_string(),
        };
        let mut items: Vec<KObject<'a>> = Vec::with_capacity(elements.len());
        let mut dep_indices: Vec<usize> = Vec::new();
        for e in elements {
            match self.resolve_or_err(e, &mut dep_indices, &make_frame) {
                Ok(v) => items.push(v),
                Err(framed) => return framed,
            }
        }
        self.reclaim_deps(idx, dep_indices);
        let arena = scope.arena;
        let allocated: &'a KObject<'a> = arena.alloc_object(build(items));
        NodeOutput::Value(allocated)
    }

    pub(super) fn schedule_list_literal(
        &mut self,
        items: Vec<ExpressionPart<'a>>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        // List-side classifier: bare `Identifier` tokens stay `Static` (the parser surfaces
        // them as already-resolved values via `resolve()`), only `Expression` parts need
        // sub-dispatch. Nested literal scheduling is handled by the shared helper.
        let elements = self.schedule_aggregate_parts(items, scope, |this, part, scope| match part {
            ExpressionPart::Expression(boxed) => {
                let sub_id = this.add(NodeWork::Dispatch(*boxed), scope);
                AggregateElement::Dep(sub_id)
            }
            other => AggregateElement::Static(other.resolve()),
        });
        self.add(NodeWork::Aggregate { elements }, scope)
    }

    /// Schedule each side of each pair in a dict literal. Sub-expressions, nested list/dict
    /// literals, and bare identifiers (which need scope lookup for Python-like name
    /// resolution on both keys and values) become `Dep` nodes; everything else inlines as
    /// `Static`. Identifier wrapping happens here rather than at parse time so the AST
    /// stays faithful to the source.
    pub(super) fn schedule_dict_literal(
        &mut self,
        pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        let mut entries: Vec<(AggregateElement<'a>, AggregateElement<'a>)> =
            Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            entries.push((
                self.schedule_dict_part(k, scope),
                self.schedule_dict_part(v, scope),
            ));
        }
        self.add(NodeWork::AggregateDict { entries }, scope)
    }

    /// Iterate `parts` and produce one `AggregateElement` per item, threading the
    /// `Expression` / `ListLiteral` / `DictLiteral` arms through `self.add(...)` and the
    /// nested aggregate schedulers (the shape both list and dict scheduling have in
    /// common). The remaining cases — list's all-other-becomes-Static, dict's
    /// Identifier-becomes-wrapped-Dispatch — diverge per call site, so each site passes
    /// its own `classify` closure for those.
    fn schedule_aggregate_parts<F>(
        &mut self,
        parts: Vec<ExpressionPart<'a>>,
        scope: &'a Scope<'a>,
        mut classify: F,
    ) -> Vec<AggregateElement<'a>>
    where
        F: FnMut(&mut Self, ExpressionPart<'a>, &'a Scope<'a>) -> AggregateElement<'a>,
    {
        let mut elements: Vec<AggregateElement<'a>> = Vec::with_capacity(parts.len());
        for part in parts {
            match part {
                ExpressionPart::ListLiteral(inner) => {
                    let nested_id = self.schedule_list_literal(inner, scope);
                    elements.push(AggregateElement::Dep(nested_id));
                }
                ExpressionPart::DictLiteral(pairs) => {
                    let nested_id = self.schedule_dict_literal(pairs, scope);
                    elements.push(AggregateElement::Dep(nested_id));
                }
                other => elements.push(classify(self, other, scope)),
            }
        }
        elements
    }

    fn schedule_dict_part(
        &mut self,
        part: ExpressionPart<'a>,
        scope: &'a Scope<'a>,
    ) -> AggregateElement<'a> {
        // Dict-side classifier: same as list's, plus the Identifier-wrap case below. The
        // schedule helper would produce the same Dep for `ListLiteral`/`DictLiteral`, so
        // we pass single-element through it for symmetry — the single-call cost is one
        // `Vec<_>` allocation, dwarfed by the per-element scheduling work itself.
        let mut out = self.schedule_aggregate_parts(vec![part], scope, |this, part, scope| match part {
            ExpressionPart::Expression(boxed) => {
                let sub_id = this.add(NodeWork::Dispatch(*boxed), scope);
                AggregateElement::Dep(sub_id)
            }
            // Bare identifier: wrap as a single-Identifier sub-expression so dispatch routes
            // through `value_lookup`. Same treatment for keys and values — Python-like name
            // resolution applies to both sides of a dict pair.
            ExpressionPart::Identifier(name) => {
                let expr = KExpression {
                    parts: vec![ExpressionPart::Identifier(name)],
                };
                let sub_id = this.add(NodeWork::Dispatch(expr), scope);
                AggregateElement::Dep(sub_id)
            }
            other => AggregateElement::Static(other.resolve()),
        });
        out.pop().expect("schedule_aggregate_parts produces exactly one element per input")
    }

    pub(super) fn run_aggregate_dict(
        &mut self,
        entries: Vec<(AggregateElement<'a>, AggregateElement<'a>)>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeOutput<'a> {
        // Paired-iteration runner: keys and values resolve in pair order, then each key is
        // converted to a KKey before insertion (the conversion can fail for non-scalar
        // keys, which short-circuits with a framed `ShapeError`). The shared piece with
        // `run_aggregate_with` is `resolve_or_err` + the frame-on-error pattern — only the
        // container shape and the KKey step are dict-specific.
        let make_frame = || Frame {
            function: "<dict>".to_string(),
            expression: "dict literal".to_string(),
        };
        let mut map: HashMap<Box<dyn Serializable + 'a>, KObject<'a>> = HashMap::new();
        let mut dep_indices: Vec<usize> = Vec::new();
        for (k_el, v_el) in entries {
            let key_obj = match self.resolve_or_err(k_el, &mut dep_indices, &make_frame) {
                Ok(v) => v,
                Err(framed) => return framed,
            };
            let value_obj = match self.resolve_or_err(v_el, &mut dep_indices, &make_frame) {
                Ok(v) => v,
                Err(framed) => return framed,
            };
            let kkey = match KKey::try_from_kobject(&key_obj) {
                Ok(k) => k,
                Err(e) => return NodeOutput::Err(e.with_frame(make_frame())),
            };
            map.insert(Box::new(kkey), value_obj);
        }
        // Reclaim Dep slots: values are deep-cloned into `map`. Errors above leave deps
        // for chain-free at finalize.
        self.reclaim_deps(idx, dep_indices);
        let arena = scope.arena;
        let allocated: &'a KObject<'a> = arena.alloc_object(KObject::Dict(Rc::new(map)));
        NodeOutput::Value(allocated)
    }

    /// Resolve one `AggregateElement` to an owned `KObject`, framing any error with the
    /// caller's `make_frame` closure on the way out. `Ok(value)` carries the deep-cloned
    /// value (and appends `dep.index()` to `dep_indices` for the success-path reclaim);
    /// `Err(framed)` carries the already-attached `NodeOutput::Err` so the caller's hot
    /// path is a single `?`-style early return without a second `with_frame` call.
    ///
    /// This is the shared piece both `run_aggregate_with` and `run_aggregate_dict` use —
    /// six match arms (three per runner) collapse into one. The frame closure is taken by
    /// reference rather than cloned per call to keep the cost in the no-error path zero.
    fn resolve_or_err(
        &self,
        element: AggregateElement<'a>,
        dep_indices: &mut Vec<usize>,
        make_frame: &dyn Fn() -> Frame,
    ) -> Result<KObject<'a>, NodeOutput<'a>> {
        match element {
            AggregateElement::Static(obj) => Ok(obj),
            AggregateElement::Dep(dep) => match self.read_result(dep) {
                Ok(v) => {
                    dep_indices.push(dep.index());
                    Ok(v.deep_clone())
                }
                Err(err) => Err(NodeOutput::Err(
                    err.clone_for_propagation().with_frame(make_frame()),
                )),
            },
        }
    }

    /// Run a `Lift { from }` shim: copy `results[from]`'s terminal output into a fresh
    /// `NodeOutput` for the current slot. The execute loop's Done arm then handles
    /// frame-aware semantics — for a frame-holding slot, `lift_kobject` deep-clones the
    /// value into the captured outer arena before the per-call frame Rc drops.
    ///
    /// Why the shim exists: a Dispatch whose body has to wait on sub-deps spawns a Bind
    /// for the real work and rewrites its own slot to `Lift { from: bind_id }`. This keeps
    /// the dispatch's result observable at its original slot index — consumers parked on
    /// the dispatch wake when the Lift writes its terminal, with no chain to chase.
    ///
    /// When `from` errored, the error is propagated *without* re-appending the slot's
    /// function frame here — the (Err, Some(frame)) arm in `execute()` adds the frame at
    /// the same site the direct-Done error path does. Same for type-mismatch on Value: the
    /// runtime return-type check happens in the Done arm.
    ///
    /// Reclamation: this slot's `node_dependencies[idx]` contains `[from]` (installed by
    /// `run_dispatch` when it wired the Lift), so the chain free at slot drop walks
    /// `idx -> from -> from's own subtree` correctly.
    ///
    /// Invariant: by the time the notify-walk wakes a Lift slot, `results[from]` is `Some`
    /// (either `Value` or `Err`). The `expect` here documents that contract — a `None`
    /// would mean the notify-walk fired without a terminal write, which is impossible by
    /// construction.
    pub(super) fn run_lift(&self, from: NodeId) -> NodeOutput<'a> {
        match self.results[from.index()]
            .as_ref()
            .expect("Lift only runs after notify wakes it from `from`'s terminal write")
        {
            NodeOutput::Value(v) => NodeOutput::Value(v),
            NodeOutput::Err(e) => NodeOutput::Err(e.clone_for_propagation()),
        }
    }

    /// Run a bound future's body and translate its `BodyResult` into a `NodeStep`. `Value`
    /// becomes `Done(Value)` — the slot stores the result. `Tail { expr, scope }` becomes
    /// `Replace { work: Dispatch(expr), scope }` — the execute loop rewrites the current
    /// slot's work (and optionally rebinds scope) and re-runs it, producing the tail-call
    /// slot reuse that keeps recursion at constant scheduler memory.
    pub(super) fn invoke_to_step(
        &mut self,
        future: KFuture<'a>,
        scope: &'a Scope<'a>,
    ) -> NodeStep<'a> {
        match future.function.invoke(scope, self, future.bundle) {
            BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
            BodyResult::Tail { expr, frame, function } => NodeStep::Replace {
                work: NodeWork::Dispatch(expr),
                frame,
                function,
            },
            BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Coverage for the dispatch-time-placeholder routing in `run_dispatch`:
    //! §1 single-Identifier short-circuit, §7 auto-wrap, §8 replay-park. Each test runs
    //! a small program through the full `Scheduler::execute` loop so the placeholder
    //! install/clear plumbing and the notify-walk are exercised end-to-end.
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::runtime::{KErrorKind, RuntimeArena};
    use crate::dispatch::values::KObject;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::expression_tree::parse;

    fn parse_one(src: &str) -> crate::parse::kexpression::KExpression<'static> {
        let mut exprs = parse(src).expect("parse should succeed");
        assert_eq!(exprs.len(), 1, "test helper expects a single expression");
        exprs.remove(0)
    }

    fn parse_all(src: &str) -> Vec<crate::parse::kexpression::KExpression<'static>> {
        parse(src).expect("parse should succeed")
    }

    /// `(some_var)` — single-Identifier dispatch — short-circuits to the value when
    /// `some_var` is already bound, without scheduling `value_lookup`.
    #[test]
    fn single_identifier_short_circuit_returns_value_when_bound() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all("LET x = 42") {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        let id = sched.add_dispatch(parse_one("(x)"), scope);
        sched.execute().unwrap();
        assert!(matches!(sched.read(id), KObject::Number(n) if *n == 42.0));
    }

    /// Submit a lookup before its binder has finalized: the §1 short-circuit installs a
    /// `Lift` parking on the binder's slot. The notify-walk wakes the lookup once the
    /// binder produces its value, and the lookup returns the bound value.
    ///
    /// Submission order is `LET y = (x); LET x = 1` — the first top-level dispatches first
    /// (its sub `(x)` parks on the not-yet-finalized `x` placeholder), then `LET x = 1`
    /// runs and wakes the parked sub.
    #[test]
    fn single_identifier_short_circuit_lift_parks_on_placeholder() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all("LET y = (x)\nLET x = 1") {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 1.0));
    }

    /// `(missing)` with no producer dispatched: §1's `Resolution::Unbound` falls through to
    /// the existing dispatch path, which routes through `value_lookup` and surfaces a
    /// structured `UnboundName` error.
    #[test]
    fn single_identifier_short_circuit_falls_through_when_unbound() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(parse_one("(missing)"), scope);
        sched.execute().unwrap();
        let err = match sched.read_result(id) {
            Err(e) => e.clone(),
            Ok(_) => panic!("missing should error"),
        };
        assert!(
            matches!(&err.kind, KErrorKind::UnboundName(name) if name == "missing"),
            "expected UnboundName, got {err}",
        );
    }

    /// `LET y = z` with `z` already bound: §7 auto-wraps the bare `z` Identifier as a
    /// sub-Dispatch, which §1's short-circuit resolves to the value. `y` ends up with `z`'s
    /// value rather than the literal string `"z"`.
    #[test]
    fn bare_identifier_in_value_slot_auto_wraps_and_resolves() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all("LET z = 7\nLET y = z") {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 7.0));
    }

    /// `LET y = z; LET z = 9` — `y`'s value-slot is bare `z`, auto-wrapped by §7. The
    /// resulting sub-Dispatch hits `z`'s placeholder via §1, parks via Lift, and resumes
    /// once `LET z = 9` finalizes.
    #[test]
    fn bare_identifier_in_value_slot_parks_when_forward_referenced() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all("LET y = z\nLET z = 9") {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 9.0));
    }

    /// Multiple bare-Identifier value-slot parts in one expression: each spawns its own
    /// sub-Dispatch which parks on its respective producer. When all producers finalize,
    /// the parent re-dispatches with both values resolved.
    ///
    /// Uses a user-defined `(ADD a: Number BY b: Number)` shape so the auto-wrap fires on
    /// both arg slots. (The plan's "PLUS a b" example uses a binary op that doesn't exist
    /// yet; the equivalent shape via a user-fn is exercised here.)
    #[test]
    fn multiple_value_slot_placeholders_park_on_distinct_producers() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all(
            "FN (ADD a: Number BY b: Number) -> Number = (a)\n\
             LET out = (ADD aa BY bb)\n\
             LET aa = 3\n\
             LET bb = 4",
        ) {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        // ADD's body just returns `a`. The value of `aa` (= 3) flows through.
        assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 3.0));
    }

    /// `f (x: 1)` before `FN f` is dispatched (in submission order). call_by_name picks up
    /// the literal `f` Identifier; §8's ref_name check sees `f` as a placeholder and parks
    /// the call slot. Once FN finalizes, the slot wakes and the call dispatches normally.
    ///
    /// Note: today the FN binder skips the placeholder install when the name is already a
    /// function in scope (overload model). To force a true forward-reference park, the
    /// callee's name must not yet be in `data` at the time the caller dispatches.
    #[test]
    fn call_by_name_replay_parks_on_forward_function_reference() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        // Call the FN by name first, then define it. Both are top-level; the call's slot
        // dispatches first, parks on the FN's placeholder, and resumes after FN finalizes.
        for e in parse_all(
            "LET out = (DOUBLE 7)\n\
             FN (DOUBLE x: Number) -> Number = (x)",
        ) {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 7.0));
    }

    /// A consumer expression that depends on multiple forward-referenced producers parks
    /// on all of them. The replay re-dispatches once *all* producers terminalize. Uses
    /// `LET out = (ADD aa BY bb)` with both `aa` and `bb` defined later; covers both §7's
    /// per-arg auto-wrap and §8's multi-producer wait via the wrapped sub-Dispatches.
    #[test]
    fn multi_producer_replay_park_waits_for_all_then_re_dispatches() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all(
            "FN (ADD a: Number BY b: Number) -> Number = (b)\n\
             LET out = (ADD aa BY bb)\n\
             LET aa = 11\n\
             LET bb = 22",
        ) {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 22.0));
    }

    /// **Miri audit-slate test** for the §1 Lift-park shape: a sub-Dispatch parks on a
    /// not-yet-finalized binder's placeholder, the binder's notify-walk wakes the parked
    /// Lift, and the Lift's `run_lift` reads the producer's terminal `Value` reference
    /// out of `results[from]`. Pins down the unsafe-adjacent shape: the `&KObject<'a>`
    /// the Lift returns is the same reference the producer wrote, not a clone — the
    /// arena lifetime contract must hold across the wake and re-run. The companion
    /// `replay_park_minimal_program_for_miri` does the same for the §8 replay-park
    /// shape.
    #[test]
    fn lift_park_minimal_program_for_miri() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        // Submission order matters: LET y first, LET z second. The §1 short-circuit
        // parks the (z) sub-Dispatch on z's placeholder, then LET z's body produces
        // the value and the parked sub wakes via notify_consumers.
        for e in parse_all("LET y = z\nLET z = 11") {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("y"), Some(KObject::Number(n)) if *n == 11.0));
    }

    /// **Miri audit-slate test** for the §8 replay-park shape: a call_by_name slot parks
    /// on a forward function-name reference (the verb is a placeholder at dispatch time).
    /// The slot's work is rewritten to `NodeWork::Dispatch(expr)`; on wake, `run_dispatch`
    /// re-runs with the same expression. Pins the placeholder install/wake plumbing
    /// against the same lifetime-erasure shape `closure_escapes_outer_call_and_remains_invocable`
    /// covers — the parked slot's scope stays valid across the wake.
    #[test]
    fn replay_park_minimal_program_for_miri() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all(
            "LET out = (DOUBLE 7)\n\
             FN (DOUBLE x: Number) -> Number = (x)",
        ) {
            sched.add_dispatch(e, scope);
        }
        sched.execute().unwrap();
        assert!(matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 7.0));
    }

    /// A producer that errors at dispatch time aborts `execute` via `?` propagation
    /// before the consumer's parked slot can resume. The consumer's binding (`y`) never
    /// happens; the run surfaces the dispatch failure instead. v1 doesn't reroute
    /// dispatch failures inside sub-Dispatches into the consumer's slot — that's a
    /// follow-up tracked under cycle-detection / structured-error work on the roadmap.
    #[test]
    fn replay_park_propagates_producer_error() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        for e in parse_all(
            "LET y = (x)\n\
             LET x = (UNDEFINED_FN)",
        ) {
            sched.add_dispatch(e, scope);
        }
        let exec_result = sched.execute();
        assert!(
            exec_result.is_err(),
            "UNDEFINED_FN dispatch failure should surface via execute",
        );
        // `y` should not have been bound — its dependency errored.
        assert!(scope.lookup("y").is_none(), "y should not bind when its dependency errors");
    }
}
