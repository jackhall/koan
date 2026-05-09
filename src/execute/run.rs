use std::collections::HashMap;
use std::rc::Rc;

use crate::dispatch::runtime::{Frame, KError};
use crate::dispatch::kfunction::{BodyResult, NodeId};
use crate::dispatch::values::KKey;
use crate::dispatch::values::KObject;
use crate::dispatch::types::{Parseable, Serializable};
use crate::dispatch::runtime::{KFuture, Scope};
use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::nodes::{AggregateElement, NodeOutput, NodeStep, NodeWork};
use super::scheduler::Scheduler;

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
