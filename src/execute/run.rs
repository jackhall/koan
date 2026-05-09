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
        // Short-circuit on the first errored dep — propagate that error rather than build a
        // partial list. Frame is generic ("<list>") because the aggregate has no signature
        // text to carry. On error we leave `node_dependencies[idx]` populated so the
        // chain-free at finalize reclaims the deps; the eager-free path below is the
        // success-case optimization.
        let list_frame = || Frame {
            function: "<list>".to_string(),
            expression: "list literal".to_string(),
        };
        let mut items: Vec<KObject<'a>> = Vec::with_capacity(elements.len());
        let mut dep_indices: Vec<usize> = Vec::new();
        for e in elements {
            match self.resolve_dep_value(e, &mut dep_indices) {
                Ok(v) => items.push(v),
                Err(err) => return NodeOutput::Err(err.with_frame(list_frame())),
            }
        }
        // Reclaim Dep slots: their values are already deep-cloned into `items` so freeing
        // the result-slot pointers is unambiguously safe.
        self.reclaim_deps(idx, dep_indices);
        let arena = scope.arena;
        let allocated: &'a KObject<'a> = arena.alloc_object(KObject::List(Rc::new(items)));
        NodeOutput::Value(allocated)
    }

    pub(super) fn schedule_list_literal(
        &mut self,
        items: Vec<ExpressionPart<'a>>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        let mut elements: Vec<AggregateElement<'a>> = Vec::with_capacity(items.len());
        for item in items {
            match item {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add(NodeWork::Dispatch(*boxed), scope);
                    elements.push(AggregateElement::Dep(sub_id));
                }
                ExpressionPart::ListLiteral(inner) => {
                    let nested_id = self.schedule_list_literal(inner, scope);
                    elements.push(AggregateElement::Dep(nested_id));
                }
                ExpressionPart::DictLiteral(pairs) => {
                    let nested_id = self.schedule_dict_literal(pairs, scope);
                    elements.push(AggregateElement::Dep(nested_id));
                }
                other => elements.push(AggregateElement::Static(other.resolve())),
            }
        }
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

    fn schedule_dict_part(
        &mut self,
        part: ExpressionPart<'a>,
        scope: &'a Scope<'a>,
    ) -> AggregateElement<'a> {
        match part {
            ExpressionPart::Expression(boxed) => {
                let sub_id = self.add(NodeWork::Dispatch(*boxed), scope);
                AggregateElement::Dep(sub_id)
            }
            ExpressionPart::ListLiteral(items) => {
                let id = self.schedule_list_literal(items, scope);
                AggregateElement::Dep(id)
            }
            ExpressionPart::DictLiteral(inner) => {
                let id = self.schedule_dict_literal(inner, scope);
                AggregateElement::Dep(id)
            }
            // Bare identifier: wrap as a single-Identifier sub-expression so dispatch routes
            // through `value_lookup`. Same treatment for keys and values — Python-like name
            // resolution applies to both sides of a dict pair.
            ExpressionPart::Identifier(name) => {
                let expr = KExpression {
                    parts: vec![ExpressionPart::Identifier(name)],
                };
                let sub_id = self.add(NodeWork::Dispatch(expr), scope);
                AggregateElement::Dep(sub_id)
            }
            other => AggregateElement::Static(other.resolve()),
        }
    }

    pub(super) fn run_aggregate_dict(
        &mut self,
        entries: Vec<(AggregateElement<'a>, AggregateElement<'a>)>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeOutput<'a> {
        let dict_frame = || Frame {
            function: "<dict>".to_string(),
            expression: "dict literal".to_string(),
        };
        let mut map: HashMap<Box<dyn Serializable + 'a>, KObject<'a>> = HashMap::new();
        let mut dep_indices: Vec<usize> = Vec::new();
        for (k_el, v_el) in entries {
            let key_obj = match self.resolve_dep_value(k_el, &mut dep_indices) {
                Ok(v) => v,
                Err(err) => return NodeOutput::Err(err.with_frame(dict_frame())),
            };
            let value_obj = match self.resolve_dep_value(v_el, &mut dep_indices) {
                Ok(v) => v,
                Err(err) => return NodeOutput::Err(err.with_frame(dict_frame())),
            };
            let kkey = match KKey::try_from_kobject(&key_obj) {
                Ok(k) => k,
                Err(e) => return NodeOutput::Err(e.with_frame(dict_frame())),
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

    /// Resolve one `AggregateElement` to an owned `KObject`. `Static` returns its value
    /// directly. `Dep` reads `results[dep]`: on success the value is deep-cloned (the
    /// Aggregate path puts owned values into a Vec or HashMap, so the original arena
    /// reference can be reclaimed) and `dep.index()` is appended to `dep_indices` for the
    /// success-path reclaim. On error the propagated `KError` is returned without a frame
    /// so each call site can attach its own context (`<list>`, `<dict>`, ...).
    fn resolve_dep_value(
        &self,
        element: AggregateElement<'a>,
        dep_indices: &mut Vec<usize>,
    ) -> Result<KObject<'a>, KError> {
        match element {
            AggregateElement::Static(obj) => Ok(obj),
            AggregateElement::Dep(dep) => match self.read_result(dep) {
                Ok(v) => {
                    dep_indices.push(dep.index());
                    Ok(v.deep_clone())
                }
                Err(err) => Err(err.clone_for_propagation()),
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
            NodeOutput::Value(v) => NodeOutput::Value(*v),
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
