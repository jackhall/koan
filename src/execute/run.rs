use std::collections::HashMap;
use std::rc::Rc;

use crate::dispatch::kerror::{Frame, KError};
use crate::dispatch::kfunction::{BodyResult, NodeId};
use crate::dispatch::kkey::KKey;
use crate::dispatch::kobject::KObject;
use crate::dispatch::ktraits::{Parseable, Serializable};
use crate::dispatch::scope::{KFuture, Scope};
use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::nodes::{AggregateDictElement, AggregateElement, NodeOutput, NodeStep, NodeWork};
use super::scheduler::Scheduler;

impl<'a> Scheduler<'a> {
    /// Walk an unresolved expression. If `lazy_candidate` matches, only schedule the
    /// eager-position `Expression` parts; the lazy positions ride through as `KExpression`
    /// data into a builtin slot typed `KExpression` (`if_then`, `FN`). Otherwise schedule
    /// every `Expression` (and `ListLiteral`) part as a sub-dispatch / aggregate dep.
    /// Returns a `NodeStep`: `Done(Value)` for an inline-dispatched body that produced a
    /// value, `Done(Forward(bind_id))` when it spawned a `Bind` to wait on subs, or
    /// `Replace { work: Dispatch(expr), .. }` when the body was a tail call (the slot gets
    /// rewritten in place by the execute loop).
    pub(super) fn run_dispatch(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
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
            return Ok(NodeStep::Done(NodeOutput::Forward(bind_id)));
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
        Ok(NodeStep::Done(NodeOutput::Forward(bind_id)))
    }

    pub(super) fn run_bind(
        &mut self,
        mut expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
    ) -> Result<NodeStep<'a>, KError> {
        // Short-circuit if any sub errored: propagate that error rather than dispatch the
        // parent. Append a frame naming the parent's expression so the trace reconstructs
        // the call chain.
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
        for (part_idx, dep_id) in subs {
            let value = self.read(dep_id);
            expr.parts[part_idx] = ExpressionPart::Future(value);
        }
        let future = scope.dispatch(expr)?;
        Ok(self.invoke_to_step(future, scope))
    }

    pub(super) fn run_aggregate(
        &self,
        elements: Vec<AggregateElement<'a>>,
        scope: &'a Scope<'a>,
    ) -> NodeOutput<'a> {
        // Short-circuit on the first errored dep — propagate that error rather than build a
        // partial list. Frame is generic ("<list>") because the aggregate has no signature
        // text to carry.
        let mut items: Vec<KObject<'a>> = Vec::with_capacity(elements.len());
        for e in elements {
            match e {
                AggregateElement::Static(obj) => items.push(obj),
                AggregateElement::Dep(dep) => match self.read_result(dep) {
                    Ok(v) => items.push(v.deep_clone()),
                    Err(err) => {
                        let frame = Frame {
                            function: "<list>".to_string(),
                            expression: "list literal".to_string(),
                        };
                        return NodeOutput::Err(err.clone_for_propagation().with_frame(frame));
                    }
                },
            }
        }
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
        let mut entries: Vec<(AggregateDictElement<'a>, AggregateDictElement<'a>)> =
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
    ) -> AggregateDictElement<'a> {
        match part {
            ExpressionPart::Expression(boxed) => {
                let sub_id = self.add(NodeWork::Dispatch(*boxed), scope);
                AggregateDictElement::Dep(sub_id)
            }
            ExpressionPart::ListLiteral(items) => {
                let id = self.schedule_list_literal(items, scope);
                AggregateDictElement::Dep(id)
            }
            ExpressionPart::DictLiteral(inner) => {
                let id = self.schedule_dict_literal(inner, scope);
                AggregateDictElement::Dep(id)
            }
            // Bare identifier: wrap as a single-Identifier sub-expression so dispatch routes
            // through `value_lookup`. Same treatment for keys and values — Python-like name
            // resolution applies to both sides of a dict pair.
            ExpressionPart::Identifier(name) => {
                let expr = KExpression {
                    parts: vec![ExpressionPart::Identifier(name)],
                };
                let sub_id = self.add(NodeWork::Dispatch(expr), scope);
                AggregateDictElement::Dep(sub_id)
            }
            other => AggregateDictElement::Static(other.resolve()),
        }
    }

    pub(super) fn run_aggregate_dict(
        &self,
        entries: Vec<(AggregateDictElement<'a>, AggregateDictElement<'a>)>,
        scope: &'a Scope<'a>,
    ) -> NodeOutput<'a> {
        let dict_frame = || Frame {
            function: "<dict>".to_string(),
            expression: "dict literal".to_string(),
        };
        let mut map: HashMap<Box<dyn Serializable + 'a>, KObject<'a>> = HashMap::new();
        for (k_el, v_el) in entries {
            let key_obj = match k_el {
                AggregateDictElement::Static(obj) => obj,
                AggregateDictElement::Dep(dep) => match self.read_result(dep) {
                    Ok(v) => v.deep_clone(),
                    Err(err) => {
                        return NodeOutput::Err(err.clone_for_propagation().with_frame(dict_frame()));
                    }
                },
            };
            let value_obj = match v_el {
                AggregateDictElement::Static(obj) => obj,
                AggregateDictElement::Dep(dep) => match self.read_result(dep) {
                    Ok(v) => v.deep_clone(),
                    Err(err) => {
                        return NodeOutput::Err(err.clone_for_propagation().with_frame(dict_frame()));
                    }
                },
            };
            let kkey = match KKey::try_from_kobject(&key_obj) {
                Ok(k) => k,
                Err(e) => return NodeOutput::Err(e.with_frame(dict_frame())),
            };
            map.insert(Box::new(kkey), value_obj);
        }
        let arena = scope.arena;
        let allocated: &'a KObject<'a> = arena.alloc_object(KObject::Dict(Rc::new(map)));
        NodeOutput::Value(allocated)
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
